use std::{fmt, fs};
use std::io::Write;

use prost::Message;
use rfr::{
    common::{TaskKind, Waker},
};

use crate::collect::{CollectedData, Data, DynamicId, SeqRecords, TaskRecords, WakeId, WakerAction};

// Perfetto clock IDs
const BUILTIN_CLOCK_REALTIME: u32 = 6;

// TrackEvent types
enum TrackEventType {
    SliceBegin,
    SliceEnd,
    Instant,
}

impl TrackEventType {
    const TYPE_SLICE_BEGIN: i32 = 1;
    const TYPE_SLICE_END: i32 = 2;
    const TYPE_INSTANT: i32 = 3;

    fn as_i32(&self) -> i32 {
        match self {
            Self::SliceBegin => Self::TYPE_SLICE_BEGIN,
            Self::SliceEnd => Self::TYPE_SLICE_END,
            Self::Instant => Self::TYPE_INSTANT,
        }
    }
}

// Sequence flags
const SEQ_INCREMENTAL_STATE_CLEARED: u32 = 1;

// Fixed UUIDs
const PROCESS_TRACK_UUID: u64 = 1;

// ---- Import generated Perfetto types ----
// These types are generated from Perfetto proto files using `cargo xtask gen-proto-perfetto`

use crate::generated::{
    track_descriptor, track_event, DebugAnnotation, ProcessDescriptor, Trace, TracePacket,
    TrackDescriptor, TrackEvent,
};

// ---- Conversion logic ----

fn abs_timestamp_to_nanos(secs: u64, subsec_micros: u32) -> u64 {
    secs.saturating_mul(1_000_000_000)
        .saturating_add(subsec_micros as u64 * 1_000)
}

fn track_uuid_for_task(task_records: &TaskRecords) -> u64 {
    task_records.task.iid.as_u64() + 2
}

fn task_track_name(task_records: &TaskRecords) -> String {
    let task = &task_records.task;
    match task.task_kind {
        TaskKind::BlockOn => "block_on".to_string(),
        TaskKind::Blocking if task.task_name.is_empty() => "Blocking".to_string(),
        _ => task.task_name.clone(),
    }
}

fn track_uuid_for_sequence(seq_records: &SeqRecords) -> u64 {
    seq_records.header.seq_id.as_u64() + 1_000_000
}

fn track_name_for_sequence(seq_records: &SeqRecords) -> String {
    format!("Sequence {}", seq_records.header.seq_id.as_u64())
}

fn annotation(name: String, value: String) -> DebugAnnotation {
    use crate::generated::debug_annotation::{NameField, Value};

    DebugAnnotation {
        name_field: Some(NameField::Name(name)),
        value: Some(Value::StringValue(value)),
        proto_value: None,
        dict_entries: vec![],
        array_values: vec![],
        proto_type_descriptor: None,
    }
}

fn waker_debug_annotations(waker: &Waker) -> Vec<DebugAnnotation> {
    match &waker.context {
        Some(iid) => vec![annotation(
            "context_iid".to_string(),
            iid.as_u64().to_string(),
        )],
        None => vec![],
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum TaskState {
    Unknown,
    Idle,
    IdleScheduled { wake_flow_id: FlowId },
    Polling,
    PollingScheduled { wake_flow_id: FlowId },
    Dropped,
}

#[derive(Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
struct FlowId(u64);

impl FlowId {
    const MASK_SPAWN: u64 = 1 << 63;
    const SHIFT_WAKE: u64 = 53;
    const MASK_WAKE: u64 = 0x3ff << Self::SHIFT_WAKE;

    fn spawn(spawned_did: DynamicId) -> FlowId {
        Self(spawned_did.as_u64() | Self::MASK_SPAWN)
    }

    fn wake(woken_did: DynamicId, wid: WakeId) -> FlowId {
        Self(woken_did.as_u64() | ((wid.as_u64() << Self::SHIFT_WAKE) & Self::MASK_WAKE))
    }

    fn max_dynamic_id() -> u64 {
        1 << Self::SHIFT_WAKE
    }
}

impl fmt::Debug for FlowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("FlowId");
        let dynamic_id = self.0 & !(Self::MASK_SPAWN | Self::MASK_WAKE);
        debug.field("dynamic_id", &dynamic_id);
        
        if self.0 & Self::MASK_SPAWN == Self::MASK_SPAWN {
            debug.field("flow", &true); 
        }

        let wake_id = (self.0 & Self::MASK_WAKE) >> Self::SHIFT_WAKE;
        if wake_id > 0 {
            debug.field("wake_id", &wake_id);
        }

        debug.finish()
    }
}

impl From<FlowId> for u64 {
    fn from(value: FlowId) -> Self {
        value.0
    }
}

fn name_field(name: &str) -> Option<track_event::NameField> {
    Some(track_event::NameField::Name(name.to_string()))
}

pub(crate) fn write_perfetto(
    collected_data: &CollectedData,
    output_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if collected_data.largest_did.as_u64() >= FlowId::max_dynamic_id() {
        return Err(format!(
            "We need at least 11 bits for flow_ids, but there were more than \
                2^53 objects recorded (largest_did={}). Perhaps check the \
                DynamicId assignment logic, this doesn't seem very likely.",
            collected_data.largest_did.as_u64()
        )
        .into());
    }

    let mut packets = Vec::new();

    // Process track descriptor
    packets.push(TracePacket {
        trusted_packet_sequence_id: Some(1),
        sequence_flags: Some(SEQ_INCREMENTAL_STATE_CLEARED),
        track_descriptor: Some(TrackDescriptor {
            uuid: Some(PROCESS_TRACK_UUID),
            static_or_dynamic_name: Some(track_descriptor::StaticOrDynamicName::Name(
                "rfr recording".to_string(),
            )),
            process: Some(ProcessDescriptor {
                pid: Some(1),
                process_name: Some("rfr recording".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    });

    for seq_records in collected_data.sequences() {
        let track_uuid = track_uuid_for_sequence(seq_records);
        let track_name = track_name_for_sequence(seq_records);

        // Track descriptor for this sequence
        packets.push(TracePacket {
            trusted_packet_sequence_id: Some(1),
            track_descriptor: Some(TrackDescriptor {
                uuid: Some(track_uuid),
                parent_uuid: Some(PROCESS_TRACK_UUID),
                static_or_dynamic_name: Some(track_descriptor::StaticOrDynamicName::Name(track_name)),
                ..Default::default()
            }),
            ..Default::default()
        });

        let packets = &mut packets;
        let mut add_packet = |timestamp_nanos| {
            packets.push(TracePacket {
                timestamp: Some(timestamp_nanos),
                timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                trusted_packet_sequence_id: Some(1),
                track_event: Some(TrackEvent {
                    r#type: Some(TrackEventType::Instant.as_i32()),
                    track_uuid: Some(track_uuid),
                    name_field: name_field("some event"),
//                    categories: vec!["task".to_string()],
//                    flow_ids,
//                    debug_annotations: vec![annotation(
//                        "spawned_iid".to_string(),
//                        spawned_iid.as_u64().to_string(),
//                    )],
                    ..Default::default()
                }),
                ..Default::default()
            });
        };

        for record in &seq_records.records {
            let timestamp_nanos =
                abs_timestamp_to_nanos(record.timestamp.secs, record.timestamp.subsec_micros);
            match &record.data {
                Data::TaskNew { .. } |
                Data::TaskPollStart { .. } |
                Data::TaskPollEnd { .. } |
                Data::TaskDrop { .. } => panic!("Task records shouldn't be added to a sequence track"),
                Data::WakerWoken { woken_by, action, wid } => add_packet(timestamp_nanos),
                Data::WakerWake { woken, action, wid } => add_packet(timestamp_nanos),
                Data::WakerClone { waker } => add_packet(timestamp_nanos),
                Data::WakerDrop { waker } => add_packet(timestamp_nanos),
                Data::Spawn { spawned_iid, by_iid } => add_packet(timestamp_nanos),
            }

        }
    }

    // Per-task track descriptors and events
    for task_records in collected_data.tasks() {
        let track_uuid = track_uuid_for_task(task_records);
        let track_name = task_track_name(task_records);

        // Track descriptor for this task
        packets.push(TracePacket {
            trusted_packet_sequence_id: Some(1),
            track_descriptor: Some(TrackDescriptor {
                uuid: Some(track_uuid),
                parent_uuid: Some(PROCESS_TRACK_UUID),
                static_or_dynamic_name: Some(track_descriptor::StaticOrDynamicName::Name(format!(
                    "{track_name} (iid={task_iid} task_id={task_id})",
                    task_iid = task_records.task.iid.as_u64(),
                    task_id = task_records.task.task_id.as_u64(),
                ))),
                ..Default::default()
            }),
            ..Default::default()
        });

        let mut state = TaskState::Unknown;
        let task = &task_records.task;
        let task_did = task_records.did;
        // Events for this task
        for record in &task_records.records {
            let timestamp_nanos =
                abs_timestamp_to_nanos(record.timestamp.secs, record.timestamp.subsec_micros);

            match &record.data {
                Data::TaskPollStart { .. } => {
                    let flow_id = if let TaskState::IdleScheduled { wake_flow_id } = state {
                        packets.push(TracePacket {
                            timestamp: Some(timestamp_nanos),
                            timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                            trusted_packet_sequence_id: Some(1),
                            track_event: Some(TrackEvent {
                                r#type: Some(TrackEventType::SliceEnd.as_i32()),
                                track_uuid: Some(track_uuid),
                                categories: vec![
                                    "scheduled".to_string(),
                                    format!("iid={}", task.iid.as_u64()),
                                ],
                                ..Default::default()
                            }),
                            ..Default::default()
                        });

                        Some(wake_flow_id)
                    } else {
                        None
                    };
                    state = TaskState::Polling;

                    let terminating_flow_ids = match flow_id {
                        Some(flow_id) => vec![flow_id.into()],
                        None => vec![],
                    };

                    let active_name = match task.task_kind {
                        TaskKind::Blocking => "active",
                        _ => "poll",
                    };

                    packets.push(TracePacket {
                        timestamp: Some(timestamp_nanos),
                        timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                        trusted_packet_sequence_id: Some(1),
                        track_event: Some(TrackEvent {
                            r#type: Some(TrackEventType::SliceBegin.as_i32()),
                            track_uuid: Some(track_uuid),
                            name_field: name_field(active_name),
                            categories: vec![
                                active_name.to_string(),
                                format!("iid={}", task.iid.as_u64()),
                            ],
                            terminating_flow_ids,
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
                Data::TaskPollEnd { .. } => {
                    state = match state {
                        TaskState::Polling => TaskState::Idle,
                        TaskState::PollingScheduled { wake_flow_id } => TaskState::IdleScheduled { wake_flow_id },
                        _ => TaskState::Idle,
                    };

                    packets.push(TracePacket {
                        timestamp: Some(timestamp_nanos),
                        timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                        trusted_packet_sequence_id: Some(1),
                        track_event: Some(TrackEvent {
                            r#type: Some(TrackEventType::SliceEnd.as_i32()),
                            track_uuid: Some(track_uuid),
                            categories: vec![
                                "poll".to_string(),
                                format!("iid={}", task.iid.as_u64()),
                            ],
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                    if let TaskState::IdleScheduled { .. } = state {
                        packets.push(TracePacket {
                            timestamp: Some(timestamp_nanos),
                            timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                            trusted_packet_sequence_id: Some(1),
                            track_event: Some(TrackEvent {
                                r#type: Some(TrackEventType::SliceBegin.as_i32()),
                                track_uuid: Some(track_uuid),
                                name_field: name_field("scheduled"),
                                categories: vec![
                                    "scheduled".to_string(),
                                    format!("iid={}", task.iid.as_u64()),
                                ],
                                ..Default::default()
                            }),
                            ..Default::default()
                        });
                    }
                }
                Data::TaskNew { .. } => {
                    state = TaskState::Idle;
                    println!("TaskNew flow_ids={:?}", vec![FlowId::spawn(task_did)]);
                    packets.push(TracePacket {
                        timestamp: Some(timestamp_nanos),
                        timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                        trusted_packet_sequence_id: Some(1),
                        track_event: Some(TrackEvent {
                            r#type: Some(TrackEventType::SliceBegin.as_i32()),
                            track_uuid: Some(track_uuid),
                            name_field: name_field(&track_name),
                            categories: vec![
                                "task".to_string(),
                                format!("iid={}", task.iid.as_u64()),
                            ],
                            terminating_flow_ids: vec![FlowId::spawn(task_did).into()],
                            debug_annotations: vec![
                                annotation(
                                    "task_kind".to_string(),
                                    format!("{:?}", task.task_kind),
                                ),
                                annotation("task_name".to_string(), task.task_name.clone()),
                                annotation(
                                    "task_id".to_string(),
                                    task.task_id.as_u64().to_string(),
                                ),
                                annotation("context".to_string(), format!("{:?}", task.context)),
                            ],
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
                Data::TaskDrop { .. } => {
                    state = TaskState::Dropped;
                    packets.push(TracePacket {
                        timestamp: Some(timestamp_nanos),
                        timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                        trusted_packet_sequence_id: Some(1),
                        track_event: Some(TrackEvent {
                            r#type: Some(TrackEventType::SliceEnd.as_i32()),
                            track_uuid: Some(track_uuid),
                            categories: vec![
                                "task".to_string(),
                                format!("iid={}", task.iid.as_u64()),
                            ],
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
                Data::WakerWoken {
                    woken_by,
                    action: _,
                    wid,
                } => {
                    // This is the WOKEN task's perspective
                    // Update state machine and create "scheduled" slice
                    let wake_flow_id = FlowId::wake(task_did, *wid);

                    let debug_annotations = match woken_by {
                        Some(iid) => vec![annotation(
                            "woken_by".to_string(),
                            iid.as_u64().to_string(),
                        )],
                        None => vec![],
                    };

                    state = match state {
                        TaskState::Idle => {
                            packets.push(TracePacket {
                                timestamp: Some(timestamp_nanos),
                                timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                                trusted_packet_sequence_id: Some(1),
                                track_event: Some(TrackEvent {
                                    r#type: Some(TrackEventType::SliceBegin.as_i32()),
                                    track_uuid: Some(track_uuid),
                                    name_field: name_field("scheduled"),
                                    categories: vec![
                                        "scheduled".to_string(),
                                        format!("iid={}", task.iid.as_u64()),
                                    ],
                                    debug_annotations,
                                    ..Default::default()
                                }),
                                ..Default::default()
                            });

                            TaskState::IdleScheduled { wake_flow_id }
                        }
                        TaskState::Polling => TaskState::PollingScheduled { wake_flow_id },
                        _ => TaskState::IdleScheduled { wake_flow_id },
                    };
                }
                Data::WakerWake {
                    woken,
                    action,
                    wid,
                } => {
                    // This is the WAKING task's perspective
                    // Create instant event with flow arrow

                    let action_name = match action {
                        WakerAction::Consume => "waker::wake",
                        WakerAction::ByRef => "waker::wake_by_ref",
                    };

                    let flow_ids = match collected_data.tasks.get(woken) {
                        Some(task_records) => vec![FlowId::wake(task_records.did, *wid).into()],
                        None => vec![],
                    };

                    packets.push(TracePacket {
                        // Because a flow is originating from this event, we shift it back by a
                        // single nanosecond so that the destination is in the future.
                        timestamp: Some(timestamp_nanos.saturating_sub(1)),
                        timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                        trusted_packet_sequence_id: Some(1),
                        track_event: Some(TrackEvent {
                            r#type: Some(TrackEventType::Instant.as_i32()),
                            track_uuid: Some(track_uuid),
                            name_field: name_field(action_name),
                            categories: vec!["waker".to_string()],
                            flow_ids,
                            debug_annotations: vec![annotation(
                                "woken".to_string(),
                                woken.as_u64().to_string(),
                            )],
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
                Data::WakerClone { waker } => {
                    packets.push(TracePacket {
                        timestamp: Some(timestamp_nanos),
                        timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                        trusted_packet_sequence_id: Some(1),
                        track_event: Some(TrackEvent {
                            r#type: Some(TrackEventType::Instant.as_i32()),
                            track_uuid: Some(track_uuid),
                            name_field: name_field("waker::clone"),
                            categories: vec!["waker".to_string()],
                            debug_annotations: waker_debug_annotations(waker),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
                Data::WakerDrop { waker } => {
                    packets.push(TracePacket {
                        timestamp: Some(timestamp_nanos),
                        timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                        trusted_packet_sequence_id: Some(1),
                        track_event: Some(TrackEvent {
                            r#type: Some(TrackEventType::Instant.as_i32()),
                            track_uuid: Some(track_uuid),
                            name_field: name_field("waker::drop"),
                            categories: vec!["waker".to_string()],
                            debug_annotations: waker_debug_annotations(waker),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
                Data::Spawn { spawned_iid, .. } => {
                    let flow_ids = match collected_data.tasks.get(spawned_iid) {
                        Some(task_records) => vec![FlowId::spawn(task_records.did).into()],
                        None => vec![],
                    };
                    packets.push(TracePacket {
                        timestamp: Some(timestamp_nanos.saturating_sub(1)),
                        timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
                        trusted_packet_sequence_id: Some(1),
                        track_event: Some(TrackEvent {
                            r#type: Some(TrackEventType::Instant.as_i32()),
                            track_uuid: Some(track_uuid),
                            name_field: name_field("task::spawn"),
                            categories: vec!["task".to_string()],
                            flow_ids,
                            debug_annotations: vec![annotation(
                                "spawned_iid".to_string(),
                                spawned_iid.as_u64().to_string(),
                            )],
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
            }
        }
    }

    let trace = Trace { packet: packets };

    let buf = trace.encode_to_vec();
    let mut file = fs::File::create(output_path)?;
    file.write_all(&buf)?;

    Ok(())
}
