use std::fs;

use eframe::{egui, epaint};
use egui_extras::StripBuilder;
use rfr::common::TaskKind;

use crate::collect::{
    chunked_recording_info, streaming_recording_info, RecordingInfo, TaskRow, TaskState,
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
        viewport: egui::ViewportBuilder::default().with_inner_size([700.0, 240.0]),
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
                                self.state.start_micros = (self.state.start_micros - scroll_x).clamp(0., max);
                            }

                            ui.shrink_clip_rect(rect);
                            for row in task_rows {
                                task_row(ui, self.state.start_micros, row);
                            }
                        });
                    });
                });
        });
    }
}

static TASK_ROW_HEIGHT: f32 = 42.;

fn task_row(ui: &mut egui::Ui, start_micros: f32, task_row: &TaskRow) -> egui::Response {
    static SECTION_HEIGHT: f32 = 20.;
    static SECTION_OFFSET: f32 = (TASK_ROW_HEIGHT - SECTION_HEIGHT) / 2.;

    let total_width = task_row.start_time.micros as f32 + task_row.total_duration();
    let desired_size = egui::vec2(total_width, 42.0);
    let (rect, response) =
        ui.allocate_exact_size(desired_size, egui::Sense::CLICK | egui::Sense::HOVER);

    let mut x = task_row.start_time.as_micros() as f32 - start_micros;
    let radius = 0.;
    if ui.is_rect_visible(rect) {
        let mut rrect = rect;
        rrect.min.x += x;
        rrect.max.x = rrect.min.x + 5.;
        rrect.min.y += 8.;
        rrect.max.y += 20.;
        ui.painter().rect(
            rrect,
            radius,
            egui::Color32::RED,
            egui::Stroke::new(0., egui::Color32::RED),
            egui::StrokeKind::Inside,
        );
        //println!("- task:");
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
