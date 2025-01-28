use rfr::{
    chunked::{Callsite, CallsiteId, ChunkedCallsites, ChunkedCallsitesWriter},
    common::{Kind, Level},
};

#[test]
fn empty_callsites() {
    let original = ChunkedCallsites::new(vec![]);
    let mut buffer = Vec::new();

    original.to_io(&mut buffer).unwrap();

    let deser = ChunkedCallsites::try_from_io(buffer.as_slice()).unwrap();

    assert_eq!(original.format_identifier, deser.format_identifier);
    assert!(deser.callsites.is_empty());
}

#[test]
fn minimal_callsite() {
    let original_callsite = Callsite {
        callsite_id: CallsiteId::from(1_u64),
        level: Level(10),
        kind: Kind::Event,
        const_fields: vec![],
        split_field_names: vec![],
    };
    let original = ChunkedCallsites::new(vec![original_callsite]);
    let mut buffer = Vec::new();

    original.to_io(&mut buffer).unwrap();

    let deser = ChunkedCallsites::try_from_io(buffer.as_slice()).unwrap();

    assert_eq!(original.format_identifier, deser.format_identifier);
    assert_eq!(1, deser.callsites.len());
    assert_eq!(original.callsites[0], deser.callsites[0]);
}

#[test]
fn callsites_writer_new() {
    let mut buffer = Vec::new();
    let identifier = {
        let writer = ChunkedCallsitesWriter::try_new(&mut buffer).unwrap();
        writer.chunked_callsites().format_identifier.clone()
    };

    let chunked_callsites = ChunkedCallsites::try_from_io(buffer.as_slice()).unwrap();

    assert_eq!(identifier, chunked_callsites.format_identifier);
    assert!(chunked_callsites.callsites.is_empty());
}

#[test]
fn callsites_writer_minimal_callsite() {
    let callsite = Callsite {
        callsite_id: CallsiteId::from(1_u64),
        level: Level(10),
        kind: Kind::Event,
        const_fields: vec![],
        split_field_names: vec![],
    };
    let mut buffer = Vec::new();
    let identifier = {
        let mut writer = ChunkedCallsitesWriter::try_new(&mut buffer).unwrap();
        writer.push_callsite(callsite.clone());
        writer.flush().unwrap();
        writer.chunked_callsites().format_identifier.clone()
    };

    let chunked_callsites = ChunkedCallsites::try_from_io(buffer.as_slice()).unwrap();

    assert_eq!(identifier, chunked_callsites.format_identifier);
    assert_eq!(1, chunked_callsites.callsites.len());
    assert_eq!(callsite, chunked_callsites.callsites[0]);
}

#[test]
fn callsties_writer_new_fails() {
    let mut buffer = [0_u8; 0];

    let new_result = ChunkedCallsitesWriter::try_new(buffer.as_mut_slice());

    assert!(new_result.is_err());
}
