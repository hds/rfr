use std::{collections::HashMap, fs};

use eframe::{egui, epaint};
use egui_extras::StripBuilder;
use rfr::common::TaskKind;

use crate::collect::{
    chunked_recording_info, streaming_recording_info, RecordingInfo, SpawnRecordKind, TaskIndex,
    TaskRow, TaskState, WakeRecordKind,
};

static TASK_ROW_HEIGHT: f32 = 42.;
static SECTION_HEIGHT: f32 = 20.;
static SECTION_OFFSET: f32 = (TASK_ROW_HEIGHT - SECTION_HEIGHT) / 2.;

pub(crate) fn start_ui(recording_file: String) -> eframe::Result {
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
        return Ok(());
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RFR Viz",
        options,
        Box::new(move |_cc| Ok(Box::new(RfrViz::new(info)))),
    )
}

#[derive(Debug)]
struct Zoom {
    nanos_per_pixel: u64,
}

impl Default for Zoom {
    fn default() -> Self {
        Self {
            nanos_per_pixel: 1_000,
        }
    }
}

impl Zoom {
    fn nanos_per_pixel(&self) -> f32 {
        self.nanos_per_pixel as f32
    }

    fn zoom_in(&mut self) {
        if self.nanos_per_pixel > 2 {
            self.nanos_per_pixel /= 2;
        } else {
            self.nanos_per_pixel = 1;
        }
    }

    fn zoom_out(&mut self) {
        self.nanos_per_pixel *= 2;
    }
}

#[derive(Debug, Default)]
struct State {
    start_nanos: f32,
    zoom: Zoom,
}

struct RfrViz {
    info: RecordingInfo,
    state: State,
}

impl RfrViz {
    fn new(info: RecordingInfo) -> Self {
        Self {
            info,
            state: Default::default(),
        }
    }
}

impl eframe::App for RfrViz {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus)) {
            self.state.zoom.zoom_in();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Minus)) {
            self.state.zoom.zoom_out();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            self.task_rows(ui);
        });
    }
}

impl RfrViz {
    fn task_rows(&mut self, ui: &mut egui::Ui) {
        let task_rows = &self.info.task_rows;
        let spacing = ui.spacing_mut();
        spacing.item_spacing = egui::vec2(0.0, 0.0);
        let mut tasks = HashMap::new();

        for row in task_rows {
            tasks.insert(row.task.iid, row.index);
        }

        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                time_bar(ui, &self.state);
            });
            egui::ScrollArea::vertical().show(ui, |ui| {
                StripBuilder::new(ui)
                    .size(egui_extras::Size::exact(100.))
                    .size(egui_extras::Size::remainder())
                    .horizontal(|mut strip| {
                        strip.cell(|ui| {
                            ui.vertical(|ui| {
                                for row in task_rows {
                                    task_label(ui, row);
                                }
                            });
                        });
                        strip.cell(|ui| {
                            ui.vertical(|ui| {
                                let available_size = ui.available_size();
                                let cursor = ui.cursor().min;
                                let mut rect = egui::Rect {
                                    min: cursor,
                                    max: cursor + available_size,
                                };
                                let clip_rect = ui.clip_rect();
                                rect.max.y = clip_rect.max.y;

                                let scroll_x = ui.input(|input| input.smooth_scroll_delta.x);
                                if scroll_x != 0. {
                                    let max = self.info.end_time.as_nanos() as f32
                                        - (self.state.zoom.nanos_per_pixel() * rect.width());
                                    let delta = scroll_x * self.state.zoom.nanos_per_pixel();
                                    self.state.start_nanos =
                                        (self.state.start_nanos - delta).clamp(0., max.max(0.));
                                }

                                ui.shrink_clip_rect(rect);
                                for row in task_rows {
                                    task_row(ui, &self.state, row);
                                }

                                for row in task_rows {
                                    spawn_line(ui, cursor, &self.state, row);
                                    waker_lines(ui, cursor, &self.state, row);
                                }
                            });
                        });
                    });
            });
        });
    }
}

fn time_bar(ui: &mut egui::Ui, state: &State) -> egui::Response {
    let available_size = ui.available_size();

    let desired_size = egui::vec2(available_size.x, 20.);
    let (rect, response) =
        ui.allocate_exact_size(desired_size, egui::Sense::CLICK | egui::Sense::HOVER);

    if ui.is_rect_visible(rect) {
        let fill_color = egui::Color32::from_gray(0xf4);
        let stroke = egui::Stroke {
            width: 1.,
            color: egui::Color32::from_gray(0xe6),
        };
        let painter = ui.painter();
        painter.rect(rect, 0., fill_color, stroke, egui::StrokeKind::Inside);
        painter.line(
            vec![
                egui::pos2(rect.min.x + 100., rect.min.y),
                egui::pos2(rect.min.x + 100., rect.max.y),
            ],
            stroke,
        );

        let ns_per_pixel = state.zoom.nanos_per_pixel();
        let start_ns = state.start_nanos;
        let width = available_size.x - 100.;
        let width_ns = ns_per_pixel * width;
        let min_tick_ns = ns_per_pixel * 200.;
        let exp = min_tick_ns.log10().ceil();
        let tick_width_ns = 10_f32.powf(exp);
        let (tick_width_ns, subticks, effective_exp) = if tick_width_ns / 4. > min_tick_ns {
            (tick_width_ns / 4., 5, exp - 2.)
        } else if tick_width_ns / 2. > min_tick_ns {
            (tick_width_ns / 2., 5, exp - 1.)
        } else {
            (tick_width_ns, 10, exp)
        };
        let effective_exp = effective_exp.min(8.);

        let rect = egui::Rect {
            // 100. is the width of the task labels
            min: rect.min + egui::vec2(100., 0.),
            max: rect.max,
        };
        ui.shrink_clip_rect(rect);

        let start_label_ns = start_ns - ((start_ns as u64) % (tick_width_ns as u64)) as f32;
        let mut label_ns = start_label_ns;
        let visuals = ui.style().interact_selectable(&response, false);
        let tick_width_pixels = tick_width_ns / ns_per_pixel;
        while label_ns <= start_ns + width_ns {
            let label_x = (label_ns - start_ns) / ns_per_pixel;
            let label_rect = egui::Rect {
                min: rect.min + egui::vec2(label_x, 0.),
                max: egui::pos2(rect.min.x + label_x + 100., rect.max.y),
            };

            ui.painter().line(
                vec![
                    epaint::pos2(label_rect.min.x, label_rect.min.y),
                    epaint::pos2(label_rect.min.x, label_rect.max.y),
                ],
                stroke,
            );
            for subtick in 1..subticks {
                let x = label_rect.min.x + subtick as f32 * (tick_width_pixels / subticks as f32);
                ui.painter().line(
                    vec![
                        epaint::pos2(x, label_rect.max.y - 5.),
                        epaint::pos2(x, label_rect.max.y),
                    ],
                    stroke,
                );
            }

            let secs = (label_ns as u64) / 1_000_000_000;
            let hours = secs / (60 * 60);
            let mins = (secs / 60) % (60 * 60);
            let secs = secs % 60;
            let maybe_hours = if hours > 0 {
                format!("{hours:02}:")
            } else {
                String::new()
            };
            let maybe_subsec = if effective_exp < 9. {
                let millis = (label_ns as u64 / 1_000_000) % 1_000;
                let maybe_micros = if effective_exp < 6. {
                    let micros = (label_ns as u64 / 1_000) % 1_000;
                    let maybe_nanos = if effective_exp < 3. {
                        let nanos = label_ns as u64 % 1_000;
                        format!(".{nanos}")
                    } else {
                        String::new()
                    };
                    format!(".{micros:03}{maybe_nanos}")
                } else {
                    String::new()
                };
                format!(".{millis:03}{maybe_micros}")
            } else {
                String::new()
            };

            let time = format!("{maybe_hours}{mins:02}:{secs:02}{maybe_subsec}");

            let mut layout_job = egui::text::LayoutJob::simple_singleline(
                time,
                egui::FontId::proportional(11.),
                visuals.text_color(),
            );
            layout_job.wrap.max_width = desired_size.x;
            layout_job.wrap.max_rows = 1;
            let galley = ui.fonts(|fonts| fonts.layout_job(layout_job));
            let text_shape = epaint::TextShape::new(
                label_rect.left_top() + egui::vec2(4., 3.),
                galley,
                visuals.text_color(),
            );
            ui.painter().add(text_shape);

            label_ns += tick_width_ns;
        }
    }

    response
}

fn task_row(ui: &mut egui::Ui, state: &State, task_row: &TaskRow) -> egui::Response {
    let total_width = task_row.start_time.micros as f32 + task_row.total_duration();
    let desired_size = egui::vec2(total_width, 42.0);
    let (rect, response) =
        ui.allocate_exact_size(desired_size, egui::Sense::CLICK | egui::Sense::HOVER);

    let mut curr_ns = task_row.start_time.as_nanos() as f32 - state.start_nanos;
    let radius = 0.;
    if ui.is_rect_visible(rect) {
        for section in &task_row.sections {
            let end_ns = curr_ns + (section.duration * 1_000) as f32;
            let x = curr_ns / state.zoom.nanos_per_pixel();
            let end_x = end_ns / state.zoom.nanos_per_pixel();
            let sec_rect = egui::Rect {
                min: egui::pos2(rect.min.x + x, rect.min.y + SECTION_OFFSET),
                max: egui::pos2(
                    rect.min.x + end_x,
                    rect.min.y + SECTION_OFFSET + SECTION_HEIGHT,
                ),
            };

            let fill_color = match section.state {
                TaskState::Active | TaskState::ActiveScheduled => {
                    egui::Color32::from_rgb(0x48, 0x9E, 0x6C)
                }
                TaskState::Idle => egui::Color32::from_rgb(0x90, 0xe8, 0xa8),
                TaskState::IdleScheduled => egui::Color32::from_rgb(0xd6, 0xe8, 0x90),
            };
            let stroke = egui::Stroke::new(1.0, fill_color);
            ui.painter().rect(
                sec_rect,
                radius,
                fill_color,
                stroke,
                egui::StrokeKind::Inside,
            );

            curr_ns = end_ns;
        }
    }

    response
}

fn task_label(ui: &mut egui::Ui, task_row: &TaskRow) -> egui::Response {
    static TASK_LABEL_WIDTH: f32 = 100.;
    let desired_size = egui::vec2(TASK_LABEL_WIDTH, TASK_ROW_HEIGHT);
    let (rect, response) =
        ui.allocate_exact_size(desired_size, egui::Sense::CLICK | egui::Sense::HOVER);

    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            0.,
            egui::Color32::from_rgb(0xcc, 0xcc, 0xcc),
            egui::Stroke::NONE,
            egui::StrokeKind::Inside,
        );

        let visuals = ui.style().interact_selectable(&response, false);

        let name = match task_row.task.task_kind {
            TaskKind::BlockOn => "block_on",
            TaskKind::Blocking if task_row.task.task_name.is_empty() => "Blocking",
            _ => task_row.task.task_name.as_str(),
        };
        let mut layout_job = egui::text::LayoutJob::simple_singleline(
            name.to_string(),
            egui::FontId::proportional(14.),
            visuals.fg_stroke.color,
        );
        layout_job.wrap.max_width = desired_size.x;
        layout_job.wrap.max_rows = 1;
        let galley = ui.fonts(|fonts| fonts.layout_job(layout_job));
        // TODO(hds): create some actually correct positioning.
        let text_shape = epaint::TextShape::new(
            rect.left_top() + egui::vec2(0., 4.),
            galley,
            visuals.text_color(),
        );
        ui.painter().add(text_shape);

        let task_id = format!("Task Id: {}", task_row.task.task_id.as_u64());
        let task_id_len = task_id.len();
        let mut layout_job = egui::text::LayoutJob::simple_singleline(
            task_id,
            egui::FontId::proportional(12.),
            visuals.fg_stroke.color,
        );
        layout_job.wrap.max_width = desired_size.x;
        layout_job.wrap.max_rows = 1;
        layout_job.sections = vec![
            egui::text::LayoutSection {
                leading_space: 0.,
                byte_range: 0..9,
                format: egui::TextFormat::simple(
                    egui::FontId::proportional(12.),
                    visuals.fg_stroke.color,
                ),
            },
            egui::text::LayoutSection {
                leading_space: 0.,
                byte_range: 9..task_id_len,
                format: egui::TextFormat::simple(
                    egui::FontId::proportional(12.),
                    egui::Color32::from_rgb(0x48, 0x9E, 0x6C),
                ),
            },
        ];
        let galley = ui.fonts(|fonts| fonts.layout_job(layout_job));
        let text_shape = epaint::TextShape::new(
            rect.left_top() + egui::vec2(0., 18.),
            galley,
            visuals.text_color(),
        );
        ui.painter().add(text_shape);
    }

    response
}

fn spawn_line(ui: &mut egui::Ui, cursor: egui::Pos2, state: &State, row: &TaskRow) {
    let Some(spawn) = &row.spawn else { return };

    let spawn_ns_offset = (row.start_time.clone() + spawn.ts).as_nanos() as f32 - state.start_nanos;
    let spawn_x = cursor.x + (spawn_ns_offset / state.zoom.nanos_per_pixel());
    let spawn_stroke: egui::Stroke = egui::Stroke {
        width: 1.,
        color: egui::Color32::from_rgb(0x09, 0xe3, 0x64),
    };
    let from_idx = match &spawn.kind {
        SpawnRecordKind::Spawn { by: Some(by_idx) } if by_idx != &row.index => Some(*by_idx),
        _ => None,
    };
    let spawn_color = egui::Color32::from_rgb(0x30, 0xba, 0x69);

    let line = LinkLine {
        line_x: spawn_x,
        task_idx: row.index,
        from_idx,
        fill_color: spawn_color,
        stroke: spawn_stroke,
        label_text: "S".into(),
    };
    link_line(ui, cursor, line);
}

fn waker_lines(ui: &mut egui::Ui, cursor: egui::Pos2, state: &State, row: &TaskRow) {
    for waking in &row.wakings {
        let wake_ns_offset =
            (row.start_time.clone() + waking.ts).as_nanos() as f32 - state.start_nanos;
        let wake_x = cursor.x + (wake_ns_offset / state.zoom.nanos_per_pixel());
        let from_idx = match &waking.kind {
            WakeRecordKind::Wake { by: Some(by_idx) }
            | WakeRecordKind::WakeByRef { by: Some(by_idx) }
                if by_idx != &row.index =>
            {
                Some(*by_idx)
            }
            _ => None,
        };
        let stroke = egui::Stroke {
            width: 1.,
            color: egui::Color32::from_rgb(0x93, 0x43, 0xdd),
        };
        let fill_color = egui::Color32::from_rgb(0xb9, 0x98, 0xd9);

        let line = LinkLine {
            line_x: wake_x,
            task_idx: row.index,
            from_idx,
            fill_color,
            stroke,
            label_text: waking.kind.to_string(),
        };
        link_line(ui, cursor, line);
    }
}

struct LinkLine {
    line_x: f32,
    task_idx: TaskIndex,
    from_idx: Option<TaskIndex>,
    fill_color: egui::Color32,
    stroke: egui::Stroke,
    label_text: String,
}

fn link_line(ui: &mut egui::Ui, cursor: egui::Pos2, line: LinkLine) {
    let row_y = cursor.y + (TASK_ROW_HEIGHT * line.task_idx.as_inner() as f32);
    let (start_y, end_y, start_arrow_points) = match line.from_idx {
        Some(from_idx) if from_idx != line.task_idx => {
            let start_y =
                cursor.y + (TASK_ROW_HEIGHT * from_idx.as_inner() as f32) + (TASK_ROW_HEIGHT / 2.);
            let end_row_offset = if from_idx > line.task_idx {
                TASK_ROW_HEIGHT - SECTION_OFFSET - 7.
            } else {
                TASK_ROW_HEIGHT - SECTION_OFFSET + 8.
            };

            let end_y = row_y + end_row_offset;

            let half_width = 6.;
            let half_height = 5.;
            let start_arrow_points = if from_idx > line.task_idx {
                vec![
                    egui::pos2(line.line_x, start_y - half_height),
                    egui::pos2(line.line_x - half_width, start_y + half_height),
                    egui::pos2(line.line_x + half_width, start_y + half_height),
                ]
            } else {
                vec![
                    egui::pos2(line.line_x, start_y + half_height),
                    egui::pos2(line.line_x - half_width, start_y - half_height),
                    egui::pos2(line.line_x + half_width, start_y - half_height),
                ]
            };

            (start_y, end_y, Some(start_arrow_points))
        }
        _ => {
            let start_y = row_y + TASK_ROW_HEIGHT - SECTION_OFFSET - 7.;
            let end_y = row_y + TASK_ROW_HEIGHT - SECTION_OFFSET + 8.;
            (start_y, end_y, None)
        }
    };

    ui.painter().line(
        vec![
            epaint::pos2(line.line_x, start_y),
            epaint::pos2(line.line_x, end_y),
        ],
        line.stroke,
    );

    let rect = egui::Rect {
        min: egui::pos2(line.line_x, row_y + TASK_ROW_HEIGHT - SECTION_OFFSET - 7.),
        max: egui::pos2(
            line.line_x + 34.,
            row_y + TASK_ROW_HEIGHT - SECTION_OFFSET + 8.,
        ),
    };
    let arrow = epaint::PathShape {
        points: vec![
            egui::pos2(line.line_x, rect.min.y),
            egui::pos2(line.line_x, rect.max.y),
            egui::pos2(line.line_x + 8., rect.min.y + rect.height() / 2.),
        ],
        closed: true,
        fill: line.stroke.color,
        stroke: epaint::PathStroke::NONE,
    };
    ui.painter().rect(
        rect,
        0.,
        line.fill_color,
        line.stroke,
        egui::StrokeKind::Inside,
    );
    let spawn_label = egui::Label::new(
        egui::RichText::new(line.label_text)
            .color(egui::Color32::WHITE)
            .strong(),
    )
    .wrap_mode(egui::TextWrapMode::Extend)
    .halign(egui::Align::Center);
    ui.put(rect, spawn_label);
    ui.painter().add(arrow);
    if let Some(points) = start_arrow_points {
        let start_arrow = epaint::PathShape {
            points,
            closed: true,
            fill: line.stroke.color,
            stroke: epaint::PathStroke::NONE,
        };
        ui.painter().add(start_arrow);
    }
}
