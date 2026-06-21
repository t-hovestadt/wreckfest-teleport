//! wreckfest-teleport CLI.
//!
//! Modes:
//!   (default / `console`)  Print live telemetry to the terminal. Use this to
//!                          confirm we are reading real Wreckfest data.
//!   `udp`                  Stream telemetry to the SimHub PC over UDP.
//!
//! Examples:
//!   wreckfest-teleport
//!   wreckfest-teleport console --rate 100
//!   wreckfest-teleport udp --target 192.168.50.2:20777
//!   wreckfest-teleport udp --target 192.168.50.2:20777 --format native

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use wreckfest_teleport::emitter::{Format, UdpEmitter};
use wreckfest_teleport::reader::{run, ReaderConfig, Status, TelemetrySink};
use wreckfest_teleport::telemetry::Telemetry;
use wreckfest_teleport::VERSION;

const DEFAULT_PORT: u16 = 20777;

fn print_help() {
    println!(
        "wreckfest-teleport {VERSION}
Automatic telemetry for Wreckfest 1 (reads real position/orientation from the
game's memory and derives motion data). No RPM/gear: Wreckfest 1 does not expose
them, so they are intentionally absent.

USAGE:
    wreckfest-teleport [MODE] [OPTIONS]

MODES:
    console            Print telemetry to the terminal (default)
    udp                Stream telemetry to the SimHub PC over UDP

OPTIONS:
    --target <IP:PORT> UDP destination (udp mode). Default port {DEFAULT_PORT}
    --rate <HZ>        Poll rate in Hz (default 100, min 60 recommended)
    --slot <N>         Player car slot (default 0; single-player is always 0)
    --format <FMT>     udp packet format: native | simhub (default simhub)
    -h, --help         Show this help
    -V, --version      Show version
"
    );
}

struct Args {
    mode: Mode,
    target: String,
    rate: u32,
    slot: u8,
    format: Format,
}

enum Mode {
    Console,
    Udp,
}

fn parse_args() -> Result<Args, String> {
    let mut mode = Mode::Console;
    let mut target: Option<String> = None;
    let mut rate: u32 = 100;
    let mut slot: u8 = 0;
    let mut format = Format::SimHub;

    let mut it = std::env::args().skip(1).peekable();

    // Optional leading mode word.
    if let Some(first) = it.peek() {
        match first.as_str() {
            "console" => {
                mode = Mode::Console;
                it.next();
            }
            "udp" => {
                mode = Mode::Udp;
                it.next();
            }
            _ => {}
        }
    }

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("wreckfest-teleport {VERSION}");
                std::process::exit(0);
            }
            "--target" => {
                target = Some(it.next().ok_or("--target needs a value (IP:PORT)")?);
            }
            "--rate" => {
                rate = it
                    .next()
                    .ok_or("--rate needs a value")?
                    .parse()
                    .map_err(|_| "--rate must be a number")?;
            }
            "--slot" => {
                slot = it
                    .next()
                    .ok_or("--slot needs a value")?
                    .parse()
                    .map_err(|_| "--slot must be 0-23")?;
            }
            "--format" => {
                format = match it.next().ok_or("--format needs a value")?.as_str() {
                    "native" => Format::Native,
                    "simhub" => Format::SimHub,
                    other => return Err(format!("unknown format '{other}' (use native|simhub)")),
                };
            }
            other => return Err(format!("unknown argument '{other}' (try --help)")),
        }
    }

    // Default target: localhost on the default port if none supplied.
    let target = target.unwrap_or_else(|| format!("127.0.0.1:{DEFAULT_PORT}"));

    Ok(Args {
        mode,
        target,
        rate: rate.max(1),
        slot,
        format,
    })
}

/// Console sink: throttled human-readable output plus status lines.
struct ConsoleSink {
    last_print: Instant,
    last_status: Option<Status>,
}

impl ConsoleSink {
    fn new() -> Self {
        Self {
            last_print: Instant::now(),
            last_status: None,
        }
    }
}

impl TelemetrySink for ConsoleSink {
    fn on_status(&mut self, status: Status) {
        if self.last_status.as_ref() != Some(&status) {
            match status {
                Status::WaitingForGame => println!("[*] Waiting for Wreckfest (Wreckfest_x64.exe)..."),
                Status::Scanning => println!("[*] Game found. Scanning memory for car node..."),
                Status::Locked { matrix_address, slot } => println!(
                    "[+] LOCKED on slot {slot} @ 0x{matrix_address:X}. Streaming telemetry."
                ),
                Status::LockLost => println!("[!] Lock lost (car despawned?). Re-scanning..."),
                Status::GameClosed => println!("[*] Game closed. Returning to idle."),
            }
            self.last_status = Some(status);
        }
    }

    fn on_frame(&mut self, t: &Telemetry) {
        // Throttle to ~10 Hz for readability.
        if self.last_print.elapsed().as_millis() < 100 {
            return;
        }
        self.last_print = Instant::now();
        let kmh = t.speed * 3.6;
        println!(
            "spd {:6.1} u/s ({:6.1} km/h) | pos [{:8.1} {:8.1} {:8.1}] | \
             g lat {:+5.2} lon {:+5.2} ver {:+5.2} | yaw {:+5.2} rad/s | impact {:4.2}",
            t.speed,
            kmh,
            t.position.x,
            t.position.y,
            t.position.z,
            t.gforce.x,
            t.gforce.z,
            t.gforce.y,
            t.angular_velocity.yaw,
            t.impact,
        );
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };

    let config = ReaderConfig {
        slot: args.slot,
        poll_hz: args.rate,
    };

    // Ctrl+C -> graceful shutdown.
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let s = shutdown.clone();
        if let Err(e) = ctrlc::set_handler(move || s.store(true, Ordering::Relaxed)) {
            eprintln!("warning: could not install Ctrl+C handler: {e}");
        }
    }

    println!("wreckfest-teleport {VERSION}");

    match args.mode {
        Mode::Console => {
            println!("[mode] console  (poll {} Hz, slot {})", config.poll_hz, config.slot);
            let mut sink = ConsoleSink::new();
            run(config, shutdown, &mut sink);
        }
        Mode::Udp => {
            let log_status = true;
            match UdpEmitter::new(args.target.clone(), args.format, log_status) {
                Ok(mut sink) => {
                    println!(
                        "[mode] udp -> {} ({:?}, poll {} Hz, slot {})",
                        sink.target(),
                        args.format,
                        config.poll_hz,
                        config.slot
                    );
                    if args.format == Format::SimHub {
                        eprintln!(
                            "[note] SimHub format = Codemasters extradata=3 (DiRT Rally 2.0). \
                             In SimHub, enable the DiRT Rally 2.0 game; it listens on UDP \
                             port 20777, so target that port."
                        );
                    }
                    run(config, shutdown, &mut sink);
                }
                Err(e) => {
                    eprintln!("error: could not open UDP socket to {}: {e}", args.target);
                    std::process::exit(1);
                }
            }
        }
    }

    println!("Shut down cleanly.");
}
