use std::{collections::HashMap, fs};

use eframe::{egui, epaint};
use egui_extras::StripBuilder;
use rfr::common::TaskKind;

use crate::collect::{
    chunked_recording_info, streaming_recording_info, RecordingInfo, SpawnRecordKind, TaskIndex,
    TaskRow, TaskState, WakeRecordKind,
};

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

#[derive(Debug, Default)]
struct State {
    start_micros: f32,
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
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.task_rows(ui);
            });
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

        ui.horizontal(|ui| {
            StripBuilder::new(ui)
                .size(egui_extras::Size::exact(100.))
                .size(egui_extras::Size::remainder())
                //.size(egui_extras::Size::exact(width))
                .horizontal(|mut strip| {
                    strip.cell(|ui| {
                        //          println!("label cell: ...");
                        //self.task_label(ui, row);
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
                                let max = self.info.end_time.as_micros() as f32 - rect.width();
                                self.state.start_micros =
                                    (self.state.start_micros - scroll_x).clamp(0., max);
                            }

                            ui.shrink_clip_rect(rect);
                            for row in task_rows {
                                task_row(ui, self.state.start_micros, row);
                            }

                            for row in task_rows {
                                spawn_line(ui, cursor, self.state.start_micros, row);
                                waker_lines(ui, cursor, self.state.start_micros, row);
                            }
                        });
                    });
                });
        });
    }
}

static TASK_ROW_HEIGHT: f32 = 42.;
static SECTION_HEIGHT: f32 = 20.;
static SECTION_OFFSET: f32 = (TASK_ROW_HEIGHT - SECTION_HEIGHT) / 2.;

fn task_row(ui: &mut egui::Ui, start_micros: f32, task_row: &TaskRow) -> egui::Response {
    let total_width = task_row.start_time.micros as f32 + task_row.total_duration();
    let desired_size = egui::vec2(total_width, 42.0);
    let (rect, response) =
        ui.allocate_exact_size(desired_size, egui::Sense::CLICK | egui::Sense::HOVER);

    let mut x = task_row.start_time.as_micros() as f32 - start_micros;
    let radius = 0.;
    if ui.is_rect_visible(rect) {
        for section in &task_row.sections {
            let min_x = rect.min.x + x;
            let sec_rect = egui::Rect {
                min: egui::pos2(min_x, rect.min.y + SECTION_OFFSET),
                max: egui::pos2(
                    min_x + section.duration as f32,
                    rect.min.y + SECTION_OFFSET + SECTION_HEIGHT,
                ),
            };
            //            let mut draw_rect = rect;
            //            draw_rect.min.x += x;
            //            draw_rect.max.x = draw_rect.min.x + section.duration as f32;
            //            draw_rect.min.y += 8.;
            //            draw_rect.max.y -= 8.;
            x += section.duration as f32;
            //println!("  - rect: {rect:?}, draw_rect: {sec_rect:?}");

            let fill_color = match section.state {
                TaskState::Active | TaskState::ActiveScheduled => {
                    egui::Color32::from_rgb(0x48, 0x9E, 0x6C)
                }
                TaskState::Idle | TaskState::IdleScheduled => {
                    egui::Color32::from_rgb(0x90, 0xe8, 0xa8)
                }
            };
            let stroke = egui::Stroke::new(1.0, fill_color);
            ui.painter().rect(
                sec_rect,
                radius,
                fill_color,
                stroke,
                egui::StrokeKind::Inside,
            );
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
        //println!(" --> {layout_job:?}");
        let galley = ui.fonts(|fonts| fonts.layout_job(layout_job));
        //println!("   --> {}", fonts_height);
        // TODO(hds): create some actually correct positioning.
        let text_shape = epaint::TextShape::new(
            rect.left_top() + egui::vec2(0., 4.),
            galley,
            visuals.text_color(),
        );
        //println!("   --> {:?}", text_shape.visual_bounding_rect().size());
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
        //println!(" --> {layout_job:?}");
        let galley = ui.fonts(|fonts| fonts.layout_job(layout_job));
        //println!("   --> {}", fonts_height);
        let text_shape = epaint::TextShape::new(
            rect.left_top() + egui::vec2(0., 18.),
            galley,
            visuals.text_color(),
        );
        //println!("   --> {:?}", text_shape.visual_bounding_rect().size());
        ui.painter().add(text_shape);
    }

    response
}
fn spawn_line(ui: &mut egui::Ui, cursor: egui::Pos2, start_micros: f32, row: &TaskRow) {
    let Some(spawn) = &row.spawn else { return };

    let spawn_x =
        cursor.x + (row.start_time.clone() + spawn.ts).as_micros() as f32 - start_micros;
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
    link_line(
        ui,
        cursor,
        line,
    );
}

fn waker_lines(ui: &mut egui::Ui, cursor: egui::Pos2, start_micros: f32, row: &TaskRow) {
    for waking in &row.wakings {
        let wake_x = cursor.x + (row.start_time.clone() + waking.ts).as_micros() as f32 - start_micros;
        let from_idx = match &waking.kind {
            WakeRecordKind::Wake { by: Some(by_idx) } | WakeRecordKind::WakeByRef { by: Some(by_idx) } if by_idx != &row.index => Some(*by_idx),
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
        link_line(
            ui,
            cursor,
            line,
        );
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

fn link_line(
    ui: &mut egui::Ui,
    cursor: egui::Pos2,
    line: LinkLine,
) {
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
        vec![epaint::pos2(line.line_x, start_y), epaint::pos2(line.line_x, end_y)],
        line.stroke,
    );

    let rect = egui::Rect {
        min: egui::pos2(line.line_x, row_y + TASK_ROW_HEIGHT - SECTION_OFFSET - 7.),
        max: egui::pos2(line.line_x + 34., row_y + TASK_ROW_HEIGHT - SECTION_OFFSET + 8.),
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
