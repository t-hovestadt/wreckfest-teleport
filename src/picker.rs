//! `pick` mode: a small egui window that lists every car in the session with a
//! live speed readout, so you can click your own car in multiplayer.
//!
//! Single-player needs no interaction — the local car is always slot 0, the
//! default selection. Online your slot is your lobby join order and differs
//! every session (SpaceMonkey leaves this manual for the same reason), so rather
//! than guess a number you brake for a moment and click the car whose speed
//! drops to zero. The selected car streams out as the same Codemasters
//! extradata=3 UDP packet as `udp` mode; clicking a different car re-points the
//! feed instantly, no restart.
//!
//! A background engine thread enumerates cars with
//! [`crate::scan::locate_all_cars`], reads every car's transform each tick to
//! update the per-car speed in the list, and runs the full [`Deriver`] on the
//! selected car to emit telemetry. The egui app only reads the shared car list
//! and writes the selected slot, so the GUI never blocks the reader.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::emitter::{Format, UdpEmitter};
use crate::math::{Transform, Vec3};
use crate::process::{find_process_pid, ProcessHandle};
use crate::reader::TelemetrySink;
use crate::scan::{locate_all_cars, ScanResult};
use crate::signatures::MATRIX_SIZE_BYTES;
use crate::telemetry::Deriver;

/// Engine poll period (~100 Hz), matching the headless reader.
const FRAME_PERIOD: Duration = Duration::from_millis(10);
/// Re-enumerate cars at least this often (covers cars joining/leaving).
const RESCAN_EVERY: Duration = Duration::from_millis(1500);
/// Window over which each car's display speed is measured. Long enough that the
/// game's frame-duplicated samples don't alias the readout.
const SPEED_WINDOW_SECS: f32 = 0.15;
/// Display threshold for the "moving" dot.
const MOVING_KMH: f32 = 2.0;

/// One row in the picker list.
#[derive(Clone)]
pub struct CarRow {
    pub slot: u8,
    pub speed_kmh: f32,
    pub moving: bool,
}

/// Engine lifecycle, surfaced to the GUI.
pub enum EngineStatus {
    SearchingForGame,
    Reading { cars: usize },
}

/// State shared between the engine thread and the egui app.
pub struct Shared {
    cars: Mutex<Vec<CarRow>>,
    status: Mutex<EngineStatus>,
    selected: AtomicU8,
    rescan: AtomicBool,
    shutdown: AtomicBool,
}

impl Shared {
    fn new(initial_slot: u8) -> Self {
        Self {
            cars: Mutex::new(Vec::new()),
            status: Mutex::new(EngineStatus::SearchingForGame),
            selected: AtomicU8::new(initial_slot),
            rescan: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        }
    }
}

/// Per-car speed tracker. Measures distance over a fixed window so a stationary
/// or frame-duplicated car reads a steady value instead of flickering.
struct Track {
    matrix_address: usize,
    window_pos: Vec3,
    window_at: Instant,
    primed: bool,
    speed_kmh: f32,
}

impl Track {
    fn new(matrix_address: usize) -> Self {
        Self {
            matrix_address,
            window_pos: Vec3::default(),
            window_at: Instant::now(),
            primed: false,
            speed_kmh: 0.0,
        }
    }

    fn observe(&mut self, pos: Vec3, now: Instant) {
        if !self.primed {
            self.window_pos = pos;
            self.window_at = now;
            self.primed = true;
            return;
        }
        let elapsed = (now - self.window_at).as_secs_f32();
        if elapsed >= SPEED_WINDOW_SECS {
            let dist = pos.sub(self.window_pos).length();
            let kmh = if elapsed > 0.0 { (dist / elapsed) * 3.6 } else { 0.0 };
            // Light smoothing so the number is readable while still responsive.
            self.speed_kmh = self.speed_kmh * 0.5 + kmh * 0.5;
            self.window_pos = pos;
            self.window_at = now;
        }
    }
}

/// Launch the picker: spawn the reader/emitter engine, then run the window.
/// Returns when the window closes. `initial_slot` is pre-selected (0 in SP).
pub fn run(target: String, format: Format, initial_slot: u8) -> Result<(), String> {
    let shared = Arc::new(Shared::new(initial_slot));

    let engine_shared = Arc::clone(&shared);
    let engine = thread::spawn(move || run_engine(target, format, engine_shared));

    let app_shared = Arc::clone(&shared);
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([380.0, 560.0])
            .with_min_inner_size([320.0, 360.0])
            .with_always_on_top()
            .with_title("Wreckfest Telemetry — Pick Your Car"),
        ..Default::default()
    };

    let result = eframe::run_native(
        "wreckfest-teleport-picker",
        native_options,
        Box::new(move |_cc| Box::new(PickerApp { shared: app_shared })),
    );

    // Window closed -> stop the engine and wait for it.
    shared.shutdown.store(true, Ordering::Relaxed);
    let _ = engine.join();
    result.map_err(|e| e.to_string())
}

/// Background thread: find the game, enumerate cars, track speeds, emit the
/// selected car. Mirrors the headless reader's process discipline.
fn run_engine(target: String, format: Format, shared: Arc<Shared>) {
    let mut emitter = UdpEmitter::new(target, format, false).ok();
    let mut deriver = Deriver::new();
    let mut last_selected = shared.selected.load(Ordering::Relaxed);
    let mut tracks: BTreeMap<u8, Track> = BTreeMap::new();
    let mut cars: Vec<ScanResult> = Vec::new();

    while !shared.shutdown.load(Ordering::Relaxed) {
        // 1) Find + open the game (read-only).
        let proc = match find_process_pid().and_then(|pid| ProcessHandle::open(pid).ok()) {
            Some(p) => p,
            None => {
                set_status(&shared, EngineStatus::SearchingForGame);
                *shared.cars.lock().unwrap() = Vec::new();
                tracks.clear();
                cars.clear();
                nap(&shared, Duration::from_millis(500));
                continue;
            }
        };

        // Connected: force an immediate scan and reset derivation.
        let mut last_scan = Instant::now()
            .checked_sub(Duration::from_secs(60))
            .unwrap_or_else(Instant::now);
        let mut last_emit = Instant::now();
        deriver.reset();
        tracks.clear();
        cars.clear();

        while !shared.shutdown.load(Ordering::Relaxed) {
            let frame_start = Instant::now();
            if !proc.is_alive() {
                break;
            }

            // 2) (Re)enumerate cars on request or periodically.
            let want_scan =
                shared.rescan.swap(false, Ordering::Relaxed) || last_scan.elapsed() >= RESCAN_EVERY;
            if want_scan {
                cars = locate_all_cars(&proc);
                last_scan = Instant::now();
                let present: BTreeSet<u8> = cars.iter().map(|c| c.slot).collect();
                tracks.retain(|slot, _| present.contains(slot));
                for c in &cars {
                    tracks
                        .entry(c.slot)
                        .and_modify(|t| t.matrix_address = c.matrix_address)
                        .or_insert_with(|| Track::new(c.matrix_address));
                }
            }

            // 3) Read each car: update display speed, capture the selected one.
            let now = Instant::now();
            let sel = shared.selected.load(Ordering::Relaxed);
            let mut selected_tf: Option<Transform> = None;
            for c in &cars {
                if let Some(bytes) = proc.read_exact::<MATRIX_SIZE_BYTES>(c.matrix_address) {
                    let tf = Transform::from_le_bytes(&bytes);
                    if tf.basis_looks_valid() {
                        if let Some(t) = tracks.get_mut(&c.slot) {
                            t.observe(tf.position(), now);
                        }
                        if c.slot == sel {
                            selected_tf = Some(tf);
                        }
                    }
                }
            }

            // 4) Publish the list for the GUI.
            {
                let mut rows: Vec<CarRow> = tracks
                    .iter()
                    .map(|(slot, t)| CarRow {
                        slot: *slot,
                        speed_kmh: t.speed_kmh,
                        moving: t.speed_kmh >= MOVING_KMH,
                    })
                    .collect();
                rows.sort_by_key(|r| r.slot);
                *shared.cars.lock().unwrap() = rows;
            }
            set_status(&shared, EngineStatus::Reading { cars: cars.len() });

            // 5) Emit the selected car. Reset the deriver when the pick changes
            //    so smoothing never carries across two different cars.
            if sel != last_selected {
                deriver.reset();
                last_emit = Instant::now();
                last_selected = sel;
            }
            if let Some(tf) = selected_tf {
                let dt = (now - last_emit).as_secs_f32().clamp(0.0005, 0.1);
                last_emit = now;
                let frame = deriver.update(&tf, dt);
                if let Some(e) = emitter.as_mut() {
                    e.on_frame(&frame);
                }
            }

            // 6) Hold cadence.
            if let Some(rem) = FRAME_PERIOD.checked_sub(frame_start.elapsed()) {
                nap(&shared, rem);
            }
        }
    }
}

fn set_status(shared: &Shared, status: EngineStatus) {
    *shared.status.lock().unwrap() = status;
}

/// Sleep that wakes early when shutdown is requested.
fn nap(shared: &Shared, dur: Duration) {
    let step = Duration::from_millis(25);
    let mut remaining = dur;
    while remaining > Duration::ZERO {
        if shared.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let n = step.min(remaining);
        thread::sleep(n);
        remaining = remaining.saturating_sub(n);
    }
}

struct PickerApp {
    shared: Arc<Shared>,
}

impl eframe::App for PickerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Keep the speeds live even with no mouse input.
        ctx.request_repaint_after(Duration::from_millis(50));

        let sel = self.shared.selected.load(Ordering::Relaxed);
        let rows = self.shared.cars.lock().unwrap().clone();
        let status = match &*self.shared.status.lock().unwrap() {
            EngineStatus::SearchingForGame => "Waiting for Wreckfest…".to_string(),
            EngineStatus::Reading { cars } => format!("Reading {cars} car(s)"),
        };

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Pick your car");
            ui.label(status);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Single-player is car 00 (already selected). Online, brake for a \
                     moment and click the car whose speed drops to 0 — that's you.",
                )
                .small(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Rescan (new lobby)").clicked() {
                    self.shared.rescan.store(true, Ordering::Relaxed);
                }
                ui.label(format!("streaming car {sel:02}"));
            });
            ui.separator();

            if rows.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    "No cars yet. Get to the pre-race screen with your car loaded, then Rescan.",
                );
                return;
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                for row in &rows {
                    let dot = if row.moving { '●' } else { '·' };
                    let label = format!(
                        "{dot}  car {:02}   {:>4} km/h",
                        row.slot,
                        row.speed_kmh.round() as i64
                    );
                    let text = egui::RichText::new(label).monospace().size(15.0);
                    if ui.selectable_label(row.slot == sel, text).clicked() {
                        self.shared.selected.store(row.slot, Ordering::Relaxed);
                    }
                }
            });
        });
    }
}
