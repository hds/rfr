use std::{cmp::Ordering, fs, io};

use rfr::TaskKind;

use crate::collect::{
    self, RecordingInfo, SpawnRecordKind, chunked_recording_info, streaming_recording_info,
};

pub(crate) fn generate_html(recording_file: String, name: String) {
    let out_fh = fs::File::create(format!("{name}.html")).unwrap();

    let recording_file_type = fs::metadata(recording_file.clone()).unwrap().file_type();
    let info = if recording_file_type.is_file() {
        streaming_recording_info(recording_file).unwrap()
    } else if recording_file_type.is_dir() {
        chunked_recording_info(recording_file).unwrap()
    } else {
        println!(
            "rfr-viz: could not determine type of recording: {}",
            recording_file,
        );
        return;
    };

    write_viz(out_fh, info, name);
}

fn write_viz(writer: impl io::Write, info: RecordingInfo, name: String) {
    let mut out_fh = writer;

    // TODO(hds): this scaling factor is a hack, needs to be fixed.
    let scaling_factor = 1_u64;
    let chart_time = |t: u64| t / scaling_factor;

    write!(out_fh, "{}", header(name)).unwrap();
    // 117: task_details div width
    // 150: some buffer because we're scaling down the time in a hacky way.
    write!(
        out_fh,
        "{}",
        canvas_start(chart_time(info.end_time.as_micros()) + 117 + 150)
    )
    .unwrap();
    for row in info.task_rows {
        let name = match row.task.task_kind {
            TaskKind::BlockOn => "<em>block_on</em>",
            TaskKind::Blocking if row.task.task_name.is_empty() => "<em>Blocking</em>",
            _ => row.task.task_name.as_str(),
        };

        write!(
            out_fh,
            r#"                    <div class="task">
                        <div class="task-details">
                            <div class="name">{name}</div>
                            <div class="id">Task Id: <span class="id">{task_id}</span></div>
                        </div>
                        <div class="task-timeline">
"#,
            task_id = row.task.task_id.as_u64(),
        )
        .unwrap();
        let mut sections = row.sections.iter();
        if let Some(first_section) = sections.next() {
            write!(out_fh,
                r#"                            <div title="{state}" class="task-state {state}" style="margin-left: {start_time}px; width: {length}px; max-width: {length}px;"></div>"#,
                state = first_section.state,
                start_time = chart_time(row.start_time.as_micros()),
                length = chart_time(first_section.duration),
            ).unwrap();
            for section in sections {
                write!(out_fh,
                    r#"<div title="{state}" class="task-state {state}" style="width: {length}px; max-width: {length}px;"></div>"#,
                    state = section.state,
                    length = chart_time(section.duration),
                ).unwrap();
            }
            if let Some(state) = row.last_state {
                let duration =
                    info.end_time.as_micros() - (row.start_time.as_micros() + row.total_duration());
                write!(out_fh,
                    r#"<div title="{state}" class="task-state {state}" style="width: {length}px; max-width: {length}px;"></div>"#,
                    length = chart_time(duration),
                ).unwrap();
            }
            writeln!(out_fh).unwrap();
        }

        if let Some(spawn) = row.spawn {
            let by_config = match &spawn.kind {
                SpawnRecordKind::Spawn { by: Some(by_index) } if by_index > &row.index => {
                    Some((by_index.as_inner() - row.index.as_inner(), "up", "-"))
                }
                SpawnRecordKind::Spawn { by: Some(by_index) } if by_index < &row.index => {
                    Some((row.index.as_inner() - by_index.as_inner(), "down", "+"))
                }
                SpawnRecordKind::Spawn { by: Some(_) } => None,
                _ => None,
            };
            write!(out_fh,
                   r#"                            <div class="spawn" style="left: {time}px;">{by}<div class="spawn-inner"><div class="spawn-border"></div><div class="spawn-marker"></div><div class="spawn-label">S</div></div></div>"#,
           time = chart_time((row.start_time.clone() + spawn.ts).as_micros()),
           by = match by_config {
                Some((index_diff, direction, pm,)) => format!(r#"<div class="spawn-line {direction}" style="height: calc(20px + (42px * {index_diff}) + (4px * {group_diff}) {pm} 5px);"><div class="spawn-from"></div></div>"#, group_diff = 0),
                None => "".into(),
           }
           ).unwrap();
        }

        for waking in row.wakings {
            let by_index = match waking.kind {
                collect::WakeRecordKind::Wake { by }
                | collect::WakeRecordKind::WakeByRef { by } => by,
                _ => None,
            };
            let by_config = by_index.and_then(|by_index| match by_index.cmp(&row.index) {
                Ordering::Greater => Some((by_index.as_inner() - row.index.as_inner(), "up", "-")),
                Ordering::Less => Some((row.index.as_inner() - by_index.as_inner(), "down", "+")),
                Ordering::Equal => None,
            });

            write!(out_fh,
                   r#"                            <div class="waker" style="left: {time}px;"><div class="waker-line"></div>{by}<div class="waker-inner"><div class="waker-border"></div><div class="waker-marker"></div><div class="waker-label">{label}</div></div></div>"#,
               time = chart_time((row.start_time.clone() + waking.ts).as_micros()),
               label = waking.kind,
                   by = match by_config {
                        Some((index_diff, direction, pm,)) => format!(r#"<div class="waker-line {direction}" style="height: calc(20px + (42px * {index_diff}) + (4px * {group_diff}) {pm} 5px);"><div class="waker-from"></div></div>"#, group_diff = 0),
                        None => "".into(),
                   },
            ).unwrap();
        }

        writeln!(
            out_fh,
            r#"                        </div>
                    </div>
"#
        )
        .unwrap();
    }
    write!(out_fh, "{}", canvas_end()).unwrap();
    write!(out_fh, "{}", footer()).unwrap();
}

fn header(name: String) -> String {
    format!(
        r#"<!DOCTYPE html>
    <head>
        <title>rfr: {name}</title>
{style}
    </head>
    <body>
"#,
        style = style()
    )
}

fn style() -> &'static str {
    r#"        <style>
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
                /* 20px + 2 x 11px = 42px = height of task-details including padding */
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

                z-index: 5;
            }

            div.waker:hover, div.spawn:hover {
                z-index: 10;
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

            div.waker-border, div.spawn-border {
                position: absolute;
                width: 32px;
                height: 15px;
                border-width: 1px;
                border-style: solid;
            }

            div.waker-border {
                border-color: #b998d9;
            }

            div.spawn-border {
                border-color: #30ba69;
            }

            div.waker:hover div.waker-border {
                border-color: #9343dd;
            }

            div.spawn:hover div.spawn-border {
                border-color: #09e364;
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
