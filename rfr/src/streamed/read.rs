use std::{fs, io::SeekFrom};

use crate::{
    FormatIdentifier,
    streamed::{Record, current_software_version},
};

pub fn from_file(filename: String) -> Vec<Record> {
    let mut file = fs::File::open(filename).unwrap();

    let mut buffer_vec = vec![0_u8; 1024];
    //let buffer: &mut [u8] = &mut buffer_vec;
    let mut file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);

    let Ok(mut end_pos) = file_buffer.0.seek(SeekFrom::End(0)) else {
        println!("cannot get file length");
        return Vec::new();
    };
    let Ok(_) = file_buffer.0.seek(SeekFrom::Start(0)) else {
        println!("cannot seek back to start of file");
        return Vec::new();
    };

    let (version, _): (FormatIdentifier, _) = postcard::from_io(file_buffer).unwrap();
    let current = current_software_version();
    if !current.can_read_version(&version) {
        panic!("Software version {current} cannot read file format version {version}",);
    }

    let mut records = Vec::new();

    use std::io::Seek;
    file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
    'record: for idx in 0_usize.. {
        let result = loop {
            let Ok(file_pos) = file_buffer.0.stream_position() else {
                println!("at {idx} cannot get file position");
                break 'record;
            };

            if file_pos >= end_pos {
                let Ok(new_end_pos) = file_buffer.0.seek(SeekFrom::End(0)) else {
                    println!("at {idx} cannot get file length");
                    break 'record;
                };
                if new_end_pos <= end_pos {
                    break 'record;
                }

                end_pos = new_end_pos;
                let Ok(_) = file_buffer.0.seek(SeekFrom::Start(0)) else {
                    println!("at {idx} cannot seek back to previous file position");
                    break 'record;
                };
                // Start loop from the beginning, even if this means we need to get the stream
                // position again.
                continue;
            }

            break match postcard::from_io(file_buffer) {
                Ok(result) => result,
                Err(postcard::Error::DeserializeUnexpectedEnd) => {
                    let new_size = buffer_vec.len() * 2;
                    const MAX_BUFFER_SIZE: usize = 1 << 20; // 1 MiB
                    if new_size > MAX_BUFFER_SIZE {
                        println!(
                            "excessive buffer required for element (> {MAX_BUFFER_SIZE}), skipping"
                        );
                        file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
                        continue 'record;
                    }
                    buffer_vec.resize(new_size * 2, 0);
                    if let Err(err) = file.seek(SeekFrom::Start(file_pos)) {
                        println!(
                            "Could not seek back to start of element after making buffer bigger: {err}"
                        );
                        file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
                        continue 'record;
                    }
                    file_buffer = (&mut file, &mut buffer_vec as &mut [u8]);
                    continue;
                }
                Err(err) => {
                    println!("Received error deserializing record index {idx}: {err} ({err:?})",);
                    return Vec::default();
                }
            };
        };

        records.push(result.0);
        file_buffer = (result.1.0, &mut buffer_vec as &mut [u8]);
    }

    records
}
