use std::{fmt, fs, io::Write, mem};

use prost::Message;
use rfr::{
    AbsTimestamp,
    common::{TaskKind, Waker},
};

use crate::{
    collect::{CollectedData, Data, DynamicId, SeqRecords, TaskRecords, WakeId, WakerAction},
    generated::{
        DebugAnnotation, ProcessDescriptor, Trace, TracePacket, TrackDescriptor, TrackEvent,
        track_descriptor, track_event,
    },
};

// Perfetto clock IDs
const BUILTIN_CLOCK_REALTIME: u32 = 6;

// Sequence flags
const SEQ_INCREMENTAL_STATE_CLEARED: u32 = 1;

// Fixed UUIDs
const PROCESS_TRACK_UUID: u64 = 1;

fn abs_timestamp_to_nanos(secs: u64, subsec_micros: u32) -> u64 {
    secs.saturating_mul(1_000_000_000)
        .saturating_add(subsec_micros as u64 * 1_000)
}

fn task_track_uuid(task_records: &TaskRecords) -> u64 {
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

fn sequence_track_uuid(seq_records: &SeqRecords) -> u64 {
    seq_records.header.seq_id.as_u64() + 1_000_000
}

fn sequence_track_name(seq_records: &SeqRecords) -> String {
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

#[derive(Debug)]
struct PacketAdder<'a> {
    packets: &'a mut Vec<TracePacket>,
    timestamp_nanos: u64,
    track_uuid: u64,

    timestamp_shift: Option<i64>,
    event_type: Option<track_event::Type>,
    name: Option<String>,
    categories: Vec<String>,
    debug_annotations: Vec<DebugAnnotation>,
    flow_ids: Vec<FlowId>,
    terminating_flow_ids: Vec<FlowId>,
}

impl<'a> PacketAdder<'a> {
    fn new(packets: &'a mut Vec<TracePacket>, track_uuid: u64, timestamp: AbsTimestamp) -> Self {
        let timestamp_nanos = abs_timestamp_to_nanos(timestamp.secs, timestamp.subsec_micros);
        Self {
            packets,
            timestamp_nanos,
            track_uuid,

            timestamp_shift: None,
            event_type: None,
            name: None,
            categories: Vec::new(),
            debug_annotations: Vec::new(),
            flow_ids: Vec::new(),
            terminating_flow_ids: Vec::new(),
        }
    }

    fn add_and_clear(&mut self) -> &mut Self {
        let event_type = self
            .event_type
            .take()
            .expect("The event_type must be set before adding.");
        let timestamp = match self.timestamp_shift.take() {
            Some(shift) if shift < 0 => self.timestamp_nanos.saturating_sub(-shift as u64),
            Some(shift) => self.timestamp_nanos.saturating_add(shift as u64),
            None => self.timestamp_nanos,
        };

        self.packets.push(TracePacket {
            timestamp: Some(timestamp),
            timestamp_clock_id: Some(BUILTIN_CLOCK_REALTIME),
            trusted_packet_sequence_id: Some(1),
            track_event: Some(TrackEvent {
                r#type: Some(event_type as i32),
                track_uuid: Some(self.track_uuid),
                name_field: self.name.take().map(track_event::NameField::Name),
                categories: mem::take(&mut self.categories),
                flow_ids: mem::take(&mut self.flow_ids)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                terminating_flow_ids: mem::take(&mut self.terminating_flow_ids)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                debug_annotations: mem::take(&mut self.debug_annotations.clone()),
                ..Default::default()
            }),
            ..Default::default()
        });

        self
    }

    fn timestamp_shift(&mut self, timestamp_shift: i64) -> &mut Self {
        self.timestamp_shift = Some(timestamp_shift);
        self
    }

    fn event_type(&mut self, event_type: track_event::Type) -> &mut Self {
        self.event_type = Some(event_type);
        self
    }

    fn name(&mut self, name: String) -> &mut Self {
        self.name = Some(name);
        self
    }

    fn categories(&mut self, categories: Vec<String>) -> &mut Self {
        self.categories = categories;
        self
    }

    fn debug_annotations(&mut self, debug_categories: Vec<DebugAnnotation>) -> &mut Self {
        self.debug_annotations = debug_categories;
        self
    }

    fn flow_id(&mut self, flow_id: Option<FlowId>) -> &mut Self {
        if let Some(id) = flow_id {
            self.flow_ids.push(id);
        }
        self
    }

    fn terminating_flow_id(&mut self, terminating_flow_id: Option<FlowId>) -> &mut Self {
        if let Some(id) = terminating_flow_id {
            self.terminating_flow_ids.push(id);
        }
        self
    }
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
        if seq_records.records.is_empty() {
            continue;
        }

        let track_uuid = sequence_track_uuid(seq_records);
        let track_name = sequence_track_name(seq_records);

        // Track descriptor for this sequence
        packets.push(TracePacket {
            trusted_packet_sequence_id: Some(1),
            track_descriptor: Some(TrackDescriptor {
                uuid: Some(track_uuid),
                parent_uuid: Some(PROCESS_TRACK_UUID),
                static_or_dynamic_name: Some(track_descriptor::StaticOrDynamicName::Name(
                    track_name,
                )),
                ..Default::default()
            }),
            ..Default::default()
        });

        for record in &seq_records.records {
            let mut adder = PacketAdder::new(&mut packets, track_uuid, record.timestamp.clone());

            match &record.data {
                Data::TaskNew { .. }
                | Data::TaskPollStart { .. }
                | Data::TaskPollEnd { .. }
                | Data::TaskDrop { .. } => {
                    debug_assert!(false, "Task records shouldn't be added to a sequence track");
                }
                Data::WakerWoken { .. } => {
                    debug_assert!(false, "A sequence can't be woken (it's not a task)");
                }
                Data::WakerWake { woken, action, wid } => {
                    let action_name = match action {
                        WakerAction::Consume => "waker::wake",
                        WakerAction::ByRef => "waker::wake_by_ref",
                    };
                    let flow_id = collected_data
                        .tasks
                        .get(woken)
                        .map(|task_records| FlowId::wake(task_records.did, *wid));
                    adder
                        .timestamp_shift(-1)
                        .event_type(track_event::Type::Instant)
                        .name(action_name.to_string())
                        .categories(vec!["waker".to_string()])
                        .flow_id(flow_id)
                        .debug_annotations(vec![annotation(
                            "woken".to_string(),
                            woken.as_u64().to_string(),
                        )])
                        .add_and_clear();
                }
                Data::WakerClone { waker } => {
                    adder
                        .event_type(track_event::Type::Instant)
                        .name("waker::clone".to_string())
                        .categories(vec!["waker".to_string()])
                        .debug_annotations(waker_debug_annotations(waker))
                        .add_and_clear();
                }
                Data::WakerDrop { waker } => {
                    adder
                        .event_type(track_event::Type::Instant)
                        .name("waker::drop".to_string())
                        .categories(vec!["waker".to_string()])
                        .debug_annotations(waker_debug_annotations(waker))
                        .add_and_clear();
                }
                Data::Spawn { spawned_iid, .. } => {
                    let flow_id = collected_data
                        .tasks
                        .get(spawned_iid)
                        .map(|task_records| FlowId::spawn(task_records.did));

                    adder
                        .timestamp_shift(-1)
                        .event_type(track_event::Type::Instant)
                        .name("task::spawn".to_string())
                        .categories(vec!["task".to_string()])
                        .flow_id(flow_id)
                        .debug_annotations(vec![annotation(
                            "spawned_iid".to_string(),
                            spawned_iid.as_u64().to_string(),
                        )])
                        .add_and_clear();
                }
            }
        }
    }

    // Per-task track descriptors and events
    for task_records in collected_data.tasks() {
        let track_uuid = task_track_uuid(task_records);
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

        for record in &task_records.records {
            let mut adder = PacketAdder::new(&mut packets, track_uuid, record.timestamp.clone());

            match &record.data {
                Data::TaskPollStart { .. } => {
                    let flow_id = if let TaskState::IdleScheduled { wake_flow_id } = state {
                        adder
                            .event_type(track_event::Type::SliceEnd)
                            .categories(vec![
                                "scheduled".to_string(),
                                format!("iid={}", task.iid.as_u64()),
                            ])
                            .add_and_clear();

                        Some(wake_flow_id)
                    } else {
                        None
                    };
                    state = TaskState::Polling;

                    let active_name = match task.task_kind {
                        TaskKind::Blocking => "active",
                        _ => "poll",
                    }
                    .to_string();

                    adder
                        .event_type(track_event::Type::SliceBegin)
                        .name(active_name.clone())
                        .categories(vec![active_name, format!("iid={}", task.iid.as_u64())])
                        .terminating_flow_id(flow_id)
                        .add_and_clear();
                }
                Data::TaskPollEnd { .. } => {
                    state = match state {
                        TaskState::Polling => TaskState::Idle,
                        TaskState::PollingScheduled { wake_flow_id } => {
                            TaskState::IdleScheduled { wake_flow_id }
                        }
                        _ => TaskState::Idle,
                    };

                    adder
                        .event_type(track_event::Type::SliceEnd)
                        .categories(vec![
                            "poll".to_string(),
                            format!("iid={}", task.iid.as_u64()),
                        ])
                        .add_and_clear();

                    if let TaskState::IdleScheduled { .. } = state {
                        adder
                            .event_type(track_event::Type::SliceBegin)
                            .name("scheduled".to_string())
                            .categories(vec![
                                "scheduled".to_string(),
                                format!("iid={}", task.iid.as_u64()),
                            ])
                            .add_and_clear();
                    }
                }
                Data::TaskNew { .. } => {
                    state = TaskState::Idle;

                    adder
                        .event_type(track_event::Type::SliceBegin)
                        .name(track_name.clone())
                        .categories(vec![
                            "task".to_string(),
                            format!("iid={}", task.iid.as_u64()),
                        ])
                        .terminating_flow_id(Some(FlowId::spawn(task_did)))
                        .debug_annotations(vec![
                            annotation("task_kind".to_string(), format!("{:?}", task.task_kind)),
                            annotation("task_name".to_string(), task.task_name.clone()),
                            annotation("task_id".to_string(), task.task_id.as_u64().to_string()),
                            annotation("context".to_string(), format!("{:?}", task.context)),
                        ])
                        .add_and_clear();
                }
                Data::TaskDrop { .. } => {
                    state = TaskState::Dropped;
                    adder
                        .event_type(track_event::Type::SliceEnd)
                        .categories(vec![
                            "task".to_string(),
                            format!("iid={}", task.iid.as_u64()),
                        ])
                        .add_and_clear();
                }
                Data::WakerWoken {
                    woken_by,
                    action: _,
                    wid,
                } => {
                    let wake_flow_id = FlowId::wake(task_did, *wid);

                    let debug_annotations = match woken_by {
                        Some(iid) => {
                            vec![annotation("woken_by".to_string(), iid.as_u64().to_string())]
                        }
                        None => vec![],
                    };

                    // This is the woken task's perspective
                    // Update state machine and create "scheduled" slice
                    state = match state {
                        TaskState::Idle => {
                            adder
                                .event_type(track_event::Type::SliceBegin)
                                .name("scheduled".to_string())
                                .categories(vec![
                                    "scheduled".to_string(),
                                    format!("iid={}", task.iid.as_u64()),
                                ])
                                .debug_annotations(debug_annotations)
                                .add_and_clear();

                            TaskState::IdleScheduled { wake_flow_id }
                        }
                        TaskState::Polling => TaskState::PollingScheduled { wake_flow_id },
                        _ => TaskState::IdleScheduled { wake_flow_id },
                    };
                }
                Data::WakerWake { woken, action, wid } => {
                    let action_name = match action {
                        WakerAction::Consume => "waker::wake",
                        WakerAction::ByRef => "waker::wake_by_ref",
                    };

                    let flow_id = collected_data
                        .tasks
                        .get(woken)
                        .map(|task_records| FlowId::wake(task_records.did, *wid));

                    adder
                        .timestamp_shift(-1)
                        .event_type(track_event::Type::Instant)
                        .name(action_name.to_string())
                        .categories(vec!["waker".to_string()])
                        .flow_id(flow_id)
                        .debug_annotations(vec![annotation(
                            "woken".to_string(),
                            woken.as_u64().to_string(),
                        )])
                        .add_and_clear();
                }
                Data::WakerClone { waker } => {
                    adder
                        .event_type(track_event::Type::Instant)
                        .name("waker::clone".to_string())
                        .categories(vec!["waker".to_string()])
                        .debug_annotations(waker_debug_annotations(waker))
                        .add_and_clear();
                }
                Data::WakerDrop { waker } => {
                    adder
                        .event_type(track_event::Type::Instant)
                        .name("waker::drop".to_string())
                        .categories(vec!["waker".to_string()])
                        .debug_annotations(waker_debug_annotations(waker))
                        .add_and_clear();
                }
                Data::Spawn { spawned_iid, .. } => {
                    let flow_id = collected_data
                        .tasks
                        .get(spawned_iid)
                        .map(|task_records| FlowId::spawn(task_records.did));

                    adder
                        .timestamp_shift(-1)
                        .event_type(track_event::Type::Instant)
                        .name("task::spawn".to_string())
                        .categories(vec!["task".to_string()])
                        .flow_id(flow_id)
                        .debug_annotations(vec![annotation(
                            "spawned_iid".to_string(),
                            spawned_iid.as_u64().to_string(),
                        )])
                        .add_and_clear();
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
