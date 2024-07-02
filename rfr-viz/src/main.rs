use std::{collections::BTreeMap, fmt, fs, io::Write, time::Duration};

use clap::Parser;
use rfr::rec::{self, from_file};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The path to a flight recording file
    recording_file: String,

    #[arg(short, long)]
    output_file: String,
}

fn time_from_rec_meta(meta: &rec::Meta) -> Duration {
    Duration::new(meta.timestamp_s, meta.timestamp_subsec_us * 1000)
}

struct TaskSection {
    length: u64,
    state: TaskState,
    debug: String,
}

enum TaskState {
    Active,
    Idle,
    ActiveSchedueld,
    IdleScheduled,
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Active => "active",
                Self::Idle => "idle",
                Self::ActiveSchedueld => "active",
                Self::IdleScheduled => "scheduled",
            }
        )
    }
}

struct TaskSpawn {
    time: u64,
    spawned_by: Option<rec::TaskId>,
}

enum WakerOp {
    Wake { woken_by: Option<rec::TaskId> },
    WakeByRef { woken_by: Option<rec::TaskId> },
    Clone,
    Drop,
}

struct TaskWake {
    time: u64,
    op: WakerOp,
}

struct TaskRow {
    task: rec::Task,
    sections: Vec<TaskSection>,
    spawn: Option<TaskSpawn>,
    wakings: Vec<WakerOp>,
}

impl TaskRow {
    fn new(task: rec::Task) -> Self {
        TaskRow {
            task,
            sections: Vec::new(),
            spawn: None,
            wakings: Vec::new(),
        }
    }

    fn task_start_time(&self) -> Option<u64> {
        self.spawn.as_ref().map(|s| s.time)
    }

    fn task_duration(&self) -> u64 {
        self.sections.iter().map(|section| section.length).sum()
    }
}

fn main() {
    let args = Args::parse();

    let records = from_file(args.recording_file);
    let mut out_fh = fs::File::create(args.output_file).unwrap();

    let mut tasks = BTreeMap::new();

    let Some(first) = records.first() else {
        println!("There are no records in the recording file.");
        return;
    };
    let last = records.last().unwrap();

    let start_time = time_from_rec_meta(&first.meta);
    let end_time = time_from_rec_meta(&last.meta);
    let total_time = end_time.saturating_sub(start_time).as_micros();
    debug_assert!(total_time < u64::MAX as u128, "recording time spans more than u64::MAX microseconds, which is more than 500 thousand years");
    let total_time = total_time as u64 + 117; // task details div width

    let scaling_factor = 100_u64;
    let chart_time = |t: u64| t / scaling_factor;

    for record in &records {
        let timestamp = time_from_rec_meta(&record.meta);
        let relative_time = timestamp.saturating_sub(start_time);
        let from_start = timestamp.saturating_sub(start_time).as_micros() as u64;

        match &record.event {
            rec::Event::Task(task) => {
                tasks.insert(task.task_id, TaskRow::new(task.clone()));
            }
            rec::Event::NewTask { id } => {
                tasks.entry(*id).and_modify(|task_row| {
                    debug_assert!(
                        task_row.spawn.is_none(),
                        "new task event received for task that already has a spawn marker"
                    );
                    task_row.spawn = Some(TaskSpawn {
                        time: from_start,
                        spawned_by: None,
                    });
                });
            }
            rec::Event::TaskPollStart { id } => {
                tasks.entry(*id).and_modify(|task_row| {
                    let Some(task_start_time) = task_row.spawn.as_ref().map(|s| s.time) else {
                        return;
                    };
                    let time_since_task_start = from_start.saturating_sub(task_start_time);
                    let last_section_duration =
                        time_since_task_start.saturating_sub(task_row.task_duration());
                    if let Some(last_section) = task_row.sections.last_mut() {
                        last_section.length = last_section_duration;
                    } else {
                        task_row.sections.push(TaskSection {
                            length: last_section_duration,
                            state: TaskState::Idle,
                            debug: "implicit".into(),
                        });
                    }
                    task_row.sections.push(TaskSection {
                        length: 0,
                        state: TaskState::Active,
                        debug: format!("{relative_time:?} -> {from_start}"),
                    });
                });
            }
            rec::Event::TaskPollEnd { id } => {
                tasks.entry(*id).and_modify(|task_row| {
                    let Some(task_start_time) = task_row.spawn.as_ref().map(|s| s.time) else {
                        return;
                    };
                    let time_since_task_start = from_start.saturating_sub(task_start_time);
                    let last_section_duration =
                        time_since_task_start.saturating_sub(task_row.task_duration());
                    if let Some(last_section) = task_row.sections.last_mut() {
                        last_section.length = last_section_duration;
                    } else {
                        task_row.sections.push(TaskSection {
                            length: last_section_duration,
                            state: TaskState::Active,
                            debug: "implicit".into(),
                        });
                    }
                    task_row.sections.push(TaskSection {
                        length: 0,
                        state: TaskState::Idle,
                        debug: format!("{relative_time:?} -> {from_start}"),
                    });
                });
            }
            rec::Event::TaskDrop { id } => {
                tasks.entry(*id).and_modify(|task_row| {
                    let Some(task_start_time) = task_row.spawn.as_ref().map(|s| s.time) else {
                        return;
                    };
                    if task_row.sections.len() >= 3 {
                        // The last poll start + poll end pair represent a "fake poll". There is
                        // no poll occuring, but the `runtime.spawn` span is entered and exited.
                        task_row.sections.truncate(task_row.sections.len() - 2);
                    }

                    let time_since_task_start = from_start.saturating_sub(task_start_time);
                    let last_section_duration =
                        time_since_task_start.saturating_sub(task_row.task_duration());

                    if let Some(last_section) = task_row.sections.last_mut() {
                        last_section.length = last_section_duration as u64;
                    } else {
                        task_row.sections.push(TaskSection {
                            length: last_section_duration,
                            state: TaskState::Idle,
                            debug: "implicit".into(),
                        });
                    }
                });
            }
            _ => {} //            rec::Event::WakerOp(_) => todo!(),
        }
    }

    write!(out_fh, "{}", header()).unwrap();
    write!(out_fh, "{}", canvas_start(chart_time(total_time) + 150)).unwrap();
    for task_row in tasks.values() {
        let name = match task_row.task.task_kind {
            rec::TaskKind::BlockOn => "<em>block_on</em>",
            _ => task_row.task.task_name.as_str(),
        };

        write!(out_fh,
            r#"                    <div class="task">
                        <div class="task-details">
                            <div class="name">{name}</div>
                            <div class="id">Task Id: <span class="id">{task_id}</span></div>
                        </div>
                        <div class="task-timeline">
"#,
            task_id = task_row.task.task_id.as_u64(),
        ).unwrap();
        let mut sections = task_row.sections.iter();
        if let Some(start_time) = task_row.task_start_time() {
            if let Some(first_section) = sections.next() {
                write!(out_fh,
                    r#"                            <div class="task-state {state}" debug="{debug}" style="margin-left: {start_time}px; width: {length}px; max-width: {length}px;"></div>"#,
                    state = first_section.state,
                    start_time = chart_time(start_time),
                    length = chart_time(first_section.length),
                    debug = first_section.debug
                ).unwrap();
                for section in sections {
                    write!(out_fh,
                        r#"<div class="task-state {state}" debug="{debug}" style="width: {length}px; max-width: {length}px;"></div>"#,
                        state = section.state,
                        length = chart_time(section.length),
                        debug = section.debug
                    ).unwrap();
                }
                writeln!(out_fh).unwrap();
            }
        }

        writeln!(out_fh,
            r#"                        </div>
                    </div>
"#
        ).unwrap();
    }
    write!(out_fh, "{}", canvas_end()).unwrap();
    write!(out_fh, "{}", footer()).unwrap();
}

fn header() -> &'static str {
    r#"<!DOCTYPE html>
    <head>
        <title>rfr spawn example</title>
        <style>
            body {
                font-family: sans-serif;
            }

            div.canvas {
                background-color: #eee;
                padding: 10px;
            }

            div.outer-group {
                margin-top: 4px;
                margin-bottom: 4px;
            }

            div.group {
                background-color: #ccc;
                width: 100%;
                clear: left;
                display: inline-block;
            }

            div.task {
                height: 42px;
                clear: left;
            }

            div.task-details, div.task-timeline {
                padding-top: 5px;
                padding-bottom: 5px;
            }

            div.task-details {
                background-color: #ccc;
                border-right: 1px solid #000;
                float: left;
                position: sticky;
                left: 0;
                width: 100px;
                height: 32px;
                padding-left: 8px;
                padding-right: 8px;

                z-index: 200;
            }

            div.task-details div.name {
                font-size: 12pt;
                text-overflow: ellipsis;
                overflow: hidden;
                white-space: nowrap;
            }

            div.task-details div.id {
                font-size: 10pt;
            }

            div.task-details div.id span.id {
                color: #489E6C;
            }

            div.task-timeline {
                float: left;
                height: 20px;
                /* 10px + 2 x 16px = 42px = height of task-details including padding */
                padding-top: 11px; 
                padding-bottom: 11px;
                position: relative;
            }

            div.task-state {
                background-color: #489E6C;
                height: 20px;
                display: inline-block;
                margin: 0;
            }

            div.task-state.active {
                background-color: #489E6C;
            }
            div.task-state.idle {
                background-color: #90e8a8;
            }
            div.task-state.scheduled {
                background-color: #d6e890;
            }

            div.waker, div.spawn {
                position: absolute;
                left: 0;
                top: 11px;
                height: 16px;
                width: 34px;
                padding-top: 14px;
            }

            div.waker-line, div.spawn-line {
                position: absolute;
                top: 0;

                height: 30px;
                width: 10px;
                z-index: 100;

                border-left: 1px solid #934edd;
            }

            div.spawn-line {
                border-color: #09e364;
            }

            div.waker-line.up, div.spawn-line.up {
                top: 0;
                bottom: auto;
            }

            div.waker-line.down, div.spawn-line.down {
                top: auto;
                bottom: 0;
            }

            div.waker-line > div.waker-from, div.spawn-line > div.spawn-from {
                position: absolute;
                left: -6.5px;
                bottom: 0;

                width: 0;
                height: 0;

                border-left: 6px solid transparent;
                border-right: 6px solid transparent;
            }

            div.waker-line.up > div.waker-from {
                top: auto;
                bottom: 0;

                border-bottom: 10px solid #9343dd;
                /*height: calc(<task bar height>
                            + (<task row height> * <tasks up/down to traverse>)
                            + (<group margin> * <groups up/down to traverse>)
                            - <task bar height>/4);*/
                /* NOTE: The last part is `-` for `up` */
                /*height: calc(20px + (42px * 1) + (4px * 0) - 5px);*/
            }

            div.spawn-line.up > div.spawn-from {
                top: auto;
                bottom: 0;

                border-bottom: 10px solid #09e364;
            }

            div.waker-line.down > div.waker-from {
                top: 0;
                bottom: auto;

                border-top: 10px solid #9343dd;

                /*height: calc(<task bar height>
                            + (<task row height> * <tasks up/down to traverse>)
                            + (<group margin> * <groups up/down to traverse>)
                            - <task bar height>/4);*/
                /* NOTE: The last part is `+` for `down` */
                /*height: calc(20px + (42px * 2) + (4px * 1) + 5px);*/
            }

            div.spawn-line.down > div.spawn-from {
                top: 0;
                bottom: auto;

                border-top: 10px solid #09e364;
            }


            div.waker-inner {
                background-color: #b998d9;
            }

            div.spawn-inner {
                background-color: #30ba69;
            }

            div.waker-marker, div.waker-label, div.spawn-marker, div.spawn-label {
                display: inline-block;
                vertical-align: middle
            }

            div.waker-marker, div.spawn-marker {
                width: 0;
                height: 0;
                border-top: 8px solid transparent;
                border-bottom: 8px solid transparent;
                border-left: 8px solid #9343dd;
            }

            div.spawn-marker {
                border-left-color: #09e364;
            }

            div.waker-label, div.spawn-label {
                margin-top: 2px;
                width: 20px;
                text-align: center;
                font-size: 8pt;
                font-weight: bold;
                color: #fff;
            }

        </style>
    </head>
    <body>
"#
}

fn canvas_start(total_time: u64) -> String {
    format!(
        r#"        <div class="canvas" style="width: {total_time}px;">

            <div class="outer-group">
                <div class="group">
"#
    )
}

fn canvas_end() -> &'static str {
    r#"                </div>
            </div>

        </div>
"#
}

fn footer() -> &'static str {
    r#"    </body>
</html>
"#
}
