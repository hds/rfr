use std::env;

use rfr_subscriber::rec::from_file;

fn main() {
    let mut args = env::args();
    let Some(filename) = args.nth(1) else {
        eprintln!("usage: read-file <filename>");
        return;
    };

    let records = from_file(filename);
    for (idx, record) in records.iter().enumerate() {
        println!("{idx}: {record:?}");
    }
}
