use clap::Parser;
use eframe::{egui, NativeOptions};
use image::{codecs::gif::GifDecoder, AnimationDecoder};
use std::{
    fs::File,
    path::PathBuf,
    sync::{
        mpsc::{channel, Receiver},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

macro_rules! log_time {
    ($start:expr, $msg:expr) => {
        println!("{}: {:.2?}", $msg, $start.elapsed());
    };
}

/// simple GIF overlay viewer
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    gif: PathBuf,

    #[arg(short, long, default_value_t = 1.0)]
    scale: f32,

    #[arg(short, long, default_value_t = 1.0)]
    opacity: f32,

    #[arg(long, default_value_t = 200)]
    width: u32,

    #[arg(long, default_value_t = 200)]
    height: u32,
}

struct Frame {
    texture: Arc<egui::TextureHandle>,
    delay: Duration,
}

enum LoadingMessage {
    FrameReady(usize, Vec<u8>, [usize; 2], Duration),
    LoadingComplete(usize),
}

struct GifOverlay {
    frames: Vec<Option<Frame>>,
    current_frame: usize,
    last_update: Instant,
    scale: f32,
    opacity: f32,
    frame_receiver: Receiver<LoadingMessage>,
    loading_complete: bool,
    first_frame_loaded: bool,
    // performance metric
    start_time: Instant,
    total_frame: usize,
    frames_loaded: usize,
    last_fps_update: Instant,
    frame_count: usize,
    last_memory_check: Instant,
}

impl GifOverlay {
    fn new(ctx: &egui::Context, gif_path: PathBuf, scale: f32, opacity: f32) -> Self {
        let start_time = Instant::now();
        println!("Starting GIF overlay application...");
        println!("Loading GIF from: {}", gif_path.display());

        // validate opacity
        let opacity = opacity.clamp(0.0, 1.0);
        // ensure scale is positive
        let scale = scale.max(0.1);

        let (sender, receiver) = channel();
        let gif_path_clone = gif_path.clone();

        println!("Spawning background loader thread...");

        thread::spawn(move || {
            let load_start = Instant::now();
            let file = File::open(gif_path_clone).expect("failed to open GIF file");

            println!("File opened in: {:.2?}", load_start.elapsed());

            let decoder = GifDecoder::new(file).expect("failed to create GIF decoder");
            let frames = decoder.into_frames();

            let mut frame_count = 0;
            let process_start = Instant::now();

            for (idx, frame) in frames.enumerate() {
                frame_count = idx + 1;
                let frame_start = Instant::now();

                let frame = frame.expect("failed to get frame");
                let delay = Duration::from(frame.delay());
                let buffer = frame.into_buffer();
                let size = [buffer.width() as _, buffer.height() as _];

                let pixels: Vec<u8> = buffer
                    .pixels()
                    .flat_map(|p| {
                        let alpha = (p[3] as f32 * opacity) as u8;
                        vec![p[0], p[1], p[2], alpha]
                    })
                    .collect();

                sender
                    .send(LoadingMessage::FrameReady(
                        idx,
                        pixels,
                        [size[0], size[1]],
                        delay,
                    ))
                    .expect("failed to send frame");
            }

            sender
                .send(LoadingMessage::LoadingComplete(frame_count))
                .expect("failed to send completion message");
        });

        Self {
            frames: Vec::new(),
            current_frame: 0,
            last_update: Instant::now(),
            scale,
            opacity,
            frame_receiver: receiver,
            loading_complete: false,
            first_frame_loaded: false,
            start_time,
            total_frame: 0,
            frames_loaded: 0,
            last_fps_update: Instant::now(),
            frame_count: 0,
            last_memory_check: Instant::now(),
        }
    }

    fn process_incoming_frames(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.frame_receiver.try_recv() {
            match message {
                LoadingMessage::FrameReady(idx, pixels, size, delay) => {
                    while self.frames.len() <= idx {
                        self.frames.push(None);
                    }

                    let color_image =
                        egui::ColorImage::from_rgba_unmultiplied([size[0], size[1]], &pixels);
                    let texture = ctx.load_texture(
                        format!("gif_frame_{}", idx),
                        color_image,
                        egui::TextureOptions::default(),
                    );

                    self.frames[idx] = Some(Frame {
                        texture: Arc::new(texture),
                        delay,
                    });

                    self.frames_loaded += 1;

                    if !self.first_frame_loaded {
                        self.first_frame_loaded = true;
                        log_time!(self.start_time, "First frame ready");
                    }

                    if self.total_frame > 0 {
                        println!(
                            "loading progress: {}/{} frames ({:.1}%)",
                            self.frames_loaded,
                            self.total_frame,
                            (self.frames_loaded as f32 / self.total_frame as f32) * 100.0
                        );
                    }
                }
                LoadingMessage::LoadingComplete(total_frames) => {
                    self.loading_complete = true;
                    self.total_frame = total_frames;
                    log_time!(self.start_time, "all frame loaded");
                }
            }
        }
    }

    fn get_next_available_frame(&self) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }

        let mut next = (self.current_frame + 1) % self.frames.len();
        let start = next;
        if next == start {
            return None;
        }

        Some(next)
    }

    fn update_performance_metrics(&mut self) {
        if self.last_fps_update.elapsed() >= Duration::from_secs(1) {
            let fps = self.frame_count as f32 / self.last_fps_update.elapsed().as_secs_f32();
            println!("FPS: {:.1}", fps);
            self.frame_count = 0;
            self.last_fps_update = Instant::now();
        }

        self.frame_count += 1;

        // check memory usage every 10s
        if self.last_memory_check.elapsed() >= Duration::from_secs(10) {
            if let Ok(memory) = sys_info::mem_info() {
                println!(
                    "Memory usage: {:.1}MB free out of {:.1}MB total",
                    memory.free as f64 / 1024.0,
                    memory.total as f64 / 1024.0,
                );
            }
            self.last_memory_check = Instant::now();
        }
    }
}

impl eframe::App for GifOverlay {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_incoming_frames(ctx);
        self.update_performance_metrics();

        egui::Window::new("GIF overlay")
            .frame(egui::Frame::none())
            .title_bar(false)
            .resizable(false)
            .movable(true)
            .show(ctx, |ui| {
                if self.first_frame_loaded {
                    let now = Instant::now();
                    if let Some(current_frame) = self.frames[self.current_frame].as_ref() {
                        if now.duration_since(self.last_update) >= current_frame.delay {
                            if let Some(next_frame) = self.get_next_available_frame() {
                                self.current_frame = next_frame;
                                self.last_update = now;
                            }
                        }

                        ui.image(current_frame.texture.as_ref());
                    }
                } else {
                    ui.spinner();
                }
            });

        if self.first_frame_loaded {
            if let Some(current_frame) = self.frames[self.current_frame].as_ref() {
                let time_until_next_frame = current_frame
                    .delay
                    .saturating_sub(Instant::now().duration_since(self.last_update));

                if !time_until_next_frame.is_zero() {
                    ctx.request_repaint_after(time_until_next_frame);
                }
            }
        }
    }
}

fn main() -> Result<(), eframe::Error> {
    let start_time = Instant::now();
    let args = Args::parse();

    println!("Configuration:");
    println!("  Scale: {}", args.scale);
    println!("  Opacity: {}", args.opacity);
    println!("  Window size: {}x{}", args.width, args.height);

    let options = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(true)
            .with_inner_size([args.width as f32, args.height as f32]),
        ..Default::default()
    };

    println!("Initializing application...");

    let result = eframe::run_native(
        "Gif overlay",
        options,
        Box::new(move |_cc| {
            Box::new(GifOverlay::new(
                &_cc.egui_ctx,
                args.gif,
                args.scale,
                args.opacity,
            ))
        }),
    );

    log_time!(start_time, "application terminated");
    result
}
