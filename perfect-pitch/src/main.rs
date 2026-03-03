use anyhow;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

const WINDOW_SIZE: usize = 4096;
const HOP_SIZE: usize = 1024;
const VOLUME_THRESHOLD: f32 = 0.02;
const SMOOTHING_SIZE: usize = 30;
const YIN_THRESHOLD: f32 = 0.15;

struct AudioMessage {
    has_pitch: bool,
    freq: f32,
    probability: f32,
    rms: f32,
    sample_rate: u32,
}

fn main() -> Result<(), eframe::Error> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        if let Err(e) = run_audio_engine(tx) {
            eprintln!("Audio Error: {}", e);
        }
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([500.0, 480.0])
            .with_title("絶対音感くんβ"),
        ..Default::default()
    };

    eframe::run_native(
        "絶対音感くんβ",
        options,
        Box::new(|_cc| Ok(Box::new(TunerApp::new(rx)))),
    )
}

struct TunerApp {
    receiver: Receiver<AudioMessage>,
    note_text: String,
    freq_text: String,
    target_cents: f32,
    current_cents: f32,
    amp: f32,
    probability: f32,
    detected_sample_rate: u32,
    history: VecDeque<f32>,
    always_on_top: bool,
}

impl TunerApp {
    fn new(receiver: Receiver<AudioMessage>) -> Self {
        Self {
            receiver,
            note_text: "--".to_string(),
            freq_text: "0.0 Hz".to_string(),
            target_cents: 0.0,
            current_cents: 0.0,
            amp: 0.0,
            probability: 0.0,
            detected_sample_rate: 0,
            history: VecDeque::with_capacity(SMOOTHING_SIZE),
            always_on_top: false,
        }
    }

    fn calculate_pitch_info(freq: f32) -> (String, f32) {
        if freq <= 0.0 { return ("--".to_string(), 0.0); }

        let note_names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

        let note_num = 12.0 * (freq / 440.0).log2() + 69.0;
        let rounded_note = note_num.round() as i32;
        let deviation = (note_num - rounded_note as f32) * 100.0;

        let note_idx = rounded_note.rem_euclid(12) as usize;
        let octave = (rounded_note / 12) - 1;

        (format!("{}{}", note_names[note_idx], octave), deviation)
    }
}

impl eframe::App for TunerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(msg) = self.receiver.try_iter().last() {
            self.amp = msg.rms;
            self.detected_sample_rate = msg.sample_rate;

            if msg.has_pitch {
                self.probability = msg.probability;

                self.history.push_back(msg.freq);
                if self.history.len() > SMOOTHING_SIZE {
                    self.history.pop_front();
                }

                let mut sorted_freqs: Vec<f32> = self.history.iter().cloned().collect();
                sorted_freqs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

                if !sorted_freqs.is_empty() {
                    let median_freq = sorted_freqs[sorted_freqs.len() / 2];
                    let (note, cents) = Self::calculate_pitch_info(median_freq);

                    self.note_text = note;
                    self.freq_text = format!("{:.2} Hz", median_freq);
                    self.target_cents = cents;
                }
            } else {
                if self.amp < VOLUME_THRESHOLD {
                    self.probability = 0.0;
                    if !self.history.is_empty() {
                         self.history.pop_front();
                    }
                } else {
                    self.probability *= 0.9;
                }
            }
        } else {
             self.amp *= 0.95;
        }

        self.current_cents += (self.target_cents - self.current_cents) * 0.08;

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.checkbox(&mut self.always_on_top, "常に最前面").changed() {
                    let level = if self.always_on_top {
                        egui::WindowLevel::AlwaysOnTop
                    } else {
                        egui::WindowLevel::Normal
                    };
                    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(5.0);
                ui.label(egui::RichText::new(format!("SR: {} Hz", self.detected_sample_rate)).size(10.0).color(egui::Color32::DARK_GRAY));

                ui.add_space(10.0);

                let (response, painter) = ui.allocate_painter(
                    egui::Vec2::new(360.0, 60.0),
                    egui::Sense::hover()
                );

                let rect = response.rect;
                painter.rect_filled(rect, 8.0, egui::Color32::from_gray(30));

                let center_x = rect.center().x;
                painter.line_segment(
                    [egui::Pos2::new(center_x, rect.top() + 5.0), egui::Pos2::new(center_x, rect.bottom() - 5.0)],
                    egui::Stroke::new(2.0, egui::Color32::from_gray(150)),
                );

                let width_half = rect.width() / 2.0;
                for i in [-25, 25] {
                    let x = center_x + (i as f32 / 50.0) * width_half;
                    painter.line_segment(
                        [egui::Pos2::new(x, rect.top() + 15.0), egui::Pos2::new(x, rect.bottom() - 15.0)],
                        egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
                    );
                }

                let range = 50.0;
                let clamped_val = self.current_cents.clamp(-range, range);
                let offset = (clamped_val / range) * width_half;

                let is_in_tune = self.current_cents.abs() < 5.0;
                let needle_color = if is_in_tune {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::from_rgb(255, 80, 80)
                };

                let alpha = (self.probability * 300.0).clamp(0.0, 255.0) as u8;
                let final_color = egui::Color32::from_rgba_premultiplied(
                    needle_color.r(), needle_color.g(), needle_color.b(), alpha
                );

                painter.circle_filled(
                    egui::Pos2::new(center_x + offset, rect.center().y),
                    14.0,
                    final_color
                );

                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(format!("Detune: {:+.1} cents", self.current_cents))
                    .color(egui::Color32::LIGHT_GRAY)
                );

                ui.add_space(20.0);

                let note_color = if self.probability > 0.25 && self.amp > VOLUME_THRESHOLD {
                    if is_in_tune { egui::Color32::GREEN } else { egui::Color32::WHITE }
                } else {
                    egui::Color32::from_rgb(60, 60, 60)
                };

                ui.label(
                    egui::RichText::new(&self.note_text)
                        .size(140.0)
                        .strong()
                        .color(note_color)
                );

                ui.label(
                    egui::RichText::new(&self.freq_text)
                        .size(32.0)
                        .strong()
                        .color(egui::Color32::GRAY)
                );

                ui.add_space(40.0);

                let grid = egui::Grid::new("indicators").spacing([10.0, 10.0]);
                grid.show(ui, |ui| {
                    ui.label("Confidence:");
                    let display_prob = self.probability.clamp(0.0, 1.0);
                    ui.add(egui::ProgressBar::new(display_prob).animate(false).desired_width(200.0));
                    ui.end_row();

                    ui.label("Input Level:");
                    let amp_norm = (self.amp * 5.0).clamp(0.0, 1.0);
                    ui.add(egui::ProgressBar::new(amp_norm).animate(true).desired_width(200.0));
                    ui.end_row();
                });
            });
        });

        ctx.request_repaint();
    }
}

fn run_audio_engine(sender: Sender<AudioMessage>) -> anyhow::Result<()> {
    let host = cpal::default_host();
    let device = host.default_input_device().expect("No input device found");
    let config: cpal::StreamConfig = device.default_input_config()?.into();
    let sample_rate = config.sample_rate.0 as f32;
    let sample_rate_u32 = config.sample_rate.0;

    let (audio_tx, audio_rx) = mpsc::channel::<f32>();

    let stream = device.build_input_stream(
        &config,
        move |data: &[f32], _: &_| {
            for &sample in data {
                let _ = audio_tx.send(sample);
            }
        },
        |err| eprintln!("Stream Error: {}", err),
        None,
    )?;
    stream.play()?;

    let mut ring_buffer: VecDeque<f32> = VecDeque::with_capacity(WINDOW_SIZE);

    let mut last_sample = 0.0;
    let alpha = 0.5;

    loop {
        while let Ok(sample) = audio_rx.try_recv() {
            let filtered = last_sample + alpha * (sample - last_sample);
            last_sample = filtered;

            ring_buffer.push_back(filtered);
            if ring_buffer.len() > WINDOW_SIZE {
                ring_buffer.pop_front();
            }
        }

        if ring_buffer.len() < WINDOW_SIZE {
            thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }

        let buffer: Vec<f32> = ring_buffer.iter().cloned().collect();

        let sum_sq: f32 = buffer.iter().map(|&x| x * x).sum();
        let rms = (sum_sq / buffer.len() as f32).sqrt();

        if rms > VOLUME_THRESHOLD {
            if let Some((freq, prob)) = yin_pitch_detection(&buffer, sample_rate) {
                let _ = sender.send(AudioMessage {
                    has_pitch: true,
                    freq,
                    probability: prob,
                    rms,
                    sample_rate: sample_rate_u32,
                });
            } else {
                let _ = sender.send(AudioMessage {
                    has_pitch: false,
                    freq: 0.0,
                    probability: 0.0,
                    rms,
                    sample_rate: sample_rate_u32,
                });
            }
        } else {
             let _ = sender.send(AudioMessage {
                has_pitch: false,
                freq: 0.0,
                probability: 0.0,
                rms,
                sample_rate: sample_rate_u32,
            });
        }

        if ring_buffer.len() >= WINDOW_SIZE {
            for _ in 0..HOP_SIZE {
                ring_buffer.pop_front();
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

fn yin_pitch_detection(buffer: &[f32], sample_rate: f32) -> Option<(f32, f32)> {
    let n = buffer.len();
    let w = n / 2;

    let min_period = (sample_rate / 4200.0) as usize;
    let max_period = (sample_rate / 25.0) as usize;

    if max_period > w { return None; }

    let mut yin_buffer = vec![0.0; max_period];

    for tau in 1..max_period {
        let mut sum = 0.0;
        for i in 0..w {
            let delta = buffer[i] - buffer[i + tau];
            sum += delta * delta;
        }
        yin_buffer[tau] = sum;
    }

    let mut running_sum = 0.0;
    yin_buffer[0] = 1.0;

    for tau in 1..max_period {
        running_sum += yin_buffer[tau];
        if running_sum < 1e-6 {
             yin_buffer[tau] = 1.0;
        } else {
             yin_buffer[tau] *= tau as f32 / running_sum;
        }
    }

    let mut best_tau = 0;
    let mut found = false;

    for tau in min_period..max_period {
        if yin_buffer[tau] < YIN_THRESHOLD {
            best_tau = tau;
            while best_tau + 1 < max_period && yin_buffer[best_tau + 1] < yin_buffer[best_tau] {
                best_tau += 1;
            }
            found = true;
            break;
        }
    }

    if !found {
        let mut min_val = 100.0;
        for tau in min_period..max_period {
            if yin_buffer[tau] < min_val {
                min_val = yin_buffer[tau];
                best_tau = tau;
            }
        }
        if min_val > 0.35 { return None; }
    }

    let half_tau = best_tau / 2;
    if half_tau >= min_period {
        let current_error = yin_buffer[best_tau];
        let higher_pitch_error = yin_buffer[half_tau];

        if higher_pitch_error < current_error + 0.15 {
            best_tau = half_tau;

            let quarter_tau = half_tau / 2;
            if quarter_tau >= min_period {
                if yin_buffer[quarter_tau] < yin_buffer[best_tau] + 0.15 {
                    best_tau = quarter_tau;
                }
            }
        }
    }

    let s0 = if best_tau > 0 { yin_buffer[best_tau - 1] } else { yin_buffer[best_tau] };
    let s1 = yin_buffer[best_tau];
    let s2 = if best_tau + 1 < max_period { yin_buffer[best_tau + 1] } else { yin_buffer[best_tau] };

    let adjustment = if (2.0 * s1 - s0 - s2).abs() > 0.0001 {
        (s0 - s2) / (2.0 * (s0 - 2.0 * s1 + s2))
    } else {
        0.0
    };

    let true_tau = best_tau as f32 + adjustment;
    let freq = sample_rate / true_tau;
    let probability = (1.0 - s1).clamp(0.0, 1.0);

    Some((freq, probability))
}