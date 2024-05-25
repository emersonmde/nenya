use std::collections::VecDeque;
use std::time::{Duration, Instant};

use clap::{Arg, Command};
use eframe::egui;
use egui::ViewportBuilder;
use egui_plot::{Corner, Line, Plot};

use nenya::pid_controller::PIDController;
use nenya::RateLimiter;

fn main() {
    let matches = Command::new("Rate Limiter Simulation")
        .about("Simulates a rate limiter using a PID controller")
        .arg(
            Arg::new("base_tps")
                .short('b')
                .long("base_tps")
                .value_parser(clap::value_parser!(f64))
                .default_value("50.0")
                .help("Base TPS for the request generator"),
        )
        .arg(
            Arg::new("min_tps")
                .short('m')
                .long("min_tps")
                .value_parser(clap::value_parser!(f64))
                .default_value("1.0")
                .help("Lower bound of TPS for the rate limiter"),
        )
        .arg(
            Arg::new("max_tps")
                .short('x')
                .long("max_tps")
                .value_parser(clap::value_parser!(f64))
                .default_value("60.0")
                .help("Upper bound of TPS for the rate limiter"),
        )
        .arg(
            Arg::new("target_tps")
                .short('t')
                .long("target_tps")
                .value_parser(clap::value_parser!(f64))
                .default_value("40.0")
                .help("Target TPS for the rate limiter"),
        )
        .arg(
            Arg::new("trailing_window")
                .short('w')
                .long("trailing_window")
                .value_parser(clap::value_parser!(u64))
                .default_value("5")
                .help("Trailing window for TPS calculation (seconds)"),
        )
        .arg(
            Arg::new("duration")
                .short('d')
                .long("duration")
                .value_parser(clap::value_parser!(u64))
                .default_value("60")
                .help("Duration of the simulation (seconds)"),
        )
        .arg(
            Arg::new("amplitudes")
                .short('a')
                .long("amplitudes")
                .value_parser(clap::value_parser!(f64))
                .num_args(1..)
                .use_value_delimiter(true)
                .default_value("20.0,10.0")
                .help("Amplitudes for the sine waves"),
        )
        .arg(
            Arg::new("frequencies")
                .short('f')
                .long("frequencies")
                .value_parser(clap::value_parser!(f64))
                .num_args(1..)
                .use_value_delimiter(true)
                .default_value("0.1,0.5")
                .help("Frequencies for the sine waves"),
        )
        .arg(
            Arg::new("kp")
                .long("kp")
                .value_parser(clap::value_parser!(f64))
                .default_value("0.5")
                .help("Proportional gain for the PID controller"),
        )
        .arg(
            Arg::new("ki")
                .long("ki")
                .value_parser(clap::value_parser!(f64))
                .default_value("0.1")
                .help("Integral gain for the PID controller"),
        )
        .arg(
            Arg::new("kd")
                .long("kd")
                .value_parser(clap::value_parser!(f64))
                .default_value("0.05")
                .help("Derivative gain for the PID controller"),
        )
        .arg(
            Arg::new("error_limit")
                .long("error_limit")
                .value_parser(clap::value_parser!(f64))
                .default_value("100.0")
                .help("Error limit for the PID controller"),
        )
        .arg(
            Arg::new("error_bias")
                .long("error_bias")
                .value_parser(clap::value_parser!(f64))
                .default_value("1.5")
                .help("Bias factor for the integral term"),
        )
        .arg(
            Arg::new("output_limit")
                .long("output_limit")
                .value_parser(clap::value_parser!(f64))
                .default_value("5.0")
                .help("Output limit for the PID controller"),
        )
        .arg(
            Arg::new("update_interval")
                .long("update_interval")
                .value_parser(clap::value_parser!(u64))
                .default_value("1000")
                .help("Update interval for the PID controller (milliseconds)"),
        )
        .get_matches();

    let base_tps = *matches.get_one::<f64>("base_tps").unwrap();
    let target_tps = *matches.get_one::<f64>("target_tps").unwrap();
    let trailing_window = Duration::from_secs(*matches.get_one::<u64>("trailing_window").unwrap());
    let duration = Duration::from_secs(*matches.get_one::<u64>("duration").unwrap());

    let amplitudes: Vec<f64> = matches
        .get_many::<f64>("amplitudes")
        .unwrap()
        .copied()
        .collect();
    let frequencies: Vec<f64> = matches
        .get_many::<f64>("frequencies")
        .unwrap()
        .copied()
        .collect();

    let min_tps = *matches.get_one::<f64>("min_tps").unwrap();
    let max_tps = *matches.get_one::<f64>("max_tps").unwrap();
    let kp = *matches.get_one::<f64>("kp").unwrap();
    let ki = *matches.get_one::<f64>("ki").unwrap();
    let kd = *matches.get_one::<f64>("kd").unwrap();
    let error_limit = *matches.get_one::<f64>("error_limit").unwrap();
    let output_limit = *matches.get_one::<f64>("output_limit").unwrap();
    let update_interval =
        Duration::from_millis(*matches.get_one::<u64>("update_interval").unwrap());
    let error_bias = *matches.get_one::<f64>("error_bias").unwrap();

    let pid_controller = PIDController::new(
        target_tps,
        kp,
        ki,
        kd,
        error_limit,
        error_bias,
        output_limit,
    );
    let rate_limiter = RateLimiter::new(
        target_tps,
        min_tps,
        max_tps,
        pid_controller,
        update_interval,
    );

    let generator = RequestGenerator::new(base_tps, amplitudes, frequencies);

    let trailing_window_clone: &'static mut Duration = Box::leak(Box::new(trailing_window.clone()));
    let duration_clone: &'static mut Duration = Box::leak(Box::new(duration.clone()));
    eframe::run_native(
        "Rate Limiter Simulation",
        eframe::NativeOptions {
            viewport: ViewportBuilder::default().with_maximized(true),
            centered: true,
            ..Default::default()
        },
        Box::new(|_cc| {
            Box::new(App::new(
                rate_limiter,
                generator,
                *trailing_window_clone,
                *duration_clone,
            ))
        }),
    )
    .unwrap();
}

struct App {
    rate_limiter: RateLimiter,
    generator: RequestGenerator,
    trailing_window: Duration,
    duration: Duration,
    start: Instant,
    accepted_requests: usize,
    total_requests: usize,
    trailing_tps_data: Vec<[f64; 2]>,
    generated_tps_data: Vec<[f64; 2]>,
    target_tps_data: Vec<[f64; 2]>,
    throttled_tps_data: Vec<[f64; 2]>,
    measured_tps_data: Vec<[f64; 2]>,
    accepted_request_times: VecDeque<Instant>,
    throttled_request_times: VecDeque<Instant>,
    last_time_point_added: f64,
}

impl App {
    fn new(
        rate_limiter: RateLimiter,
        generator: RequestGenerator,
        trailing_window: Duration,
        duration: Duration,
    ) -> Self {
        Self {
            rate_limiter,
            generator,
            trailing_window,
            duration,
            start: Instant::now(),
            accepted_requests: 0,
            total_requests: 0,
            trailing_tps_data: Vec::new(),
            generated_tps_data: Vec::new(),
            target_tps_data: Vec::new(),
            throttled_tps_data: Vec::new(),
            measured_tps_data: Vec::new(),
            accepted_request_times: VecDeque::new(),
            throttled_request_times: VecDeque::new(),
            last_time_point_added: 0.0,
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let elapsed_seconds = self.start.elapsed().as_secs_f64();

        if elapsed_seconds < self.duration.as_secs_f64() {
            // Generate a varying request rate using the RequestGenerator
            let generated_tps = self.generator.generate_request_rate(elapsed_seconds);
            let inter_request_delay = if generated_tps > 0.0 {
                (1000.0 / generated_tps) as u64
            } else {
                1000
            };

            let should_accept_request = self.rate_limiter.handle_request();
            self.total_requests += 1;
            let now = Instant::now();

            // Add new indicator at the end of the buffer
            if should_accept_request {
                self.accepted_requests += 1;
                self.accepted_request_times.push_back(now);
            } else {
                self.throttled_request_times.push_back(now);
            }

            // Remove old timestamps outside the trailing window
            while let Some(&time) = self.accepted_request_times.front() {
                if now.duration_since(time) > self.trailing_window {
                    self.accepted_request_times.pop_front();
                } else {
                    break;
                }
            }

            while let Some(&time) = self.throttled_request_times.front() {
                if now.duration_since(time) > self.trailing_window {
                    self.throttled_request_times.pop_front();
                } else {
                    break;
                }
            }

            let trailing_tps =
                self.accepted_request_times.len() as f64 / self.trailing_window.as_secs_f64();
            let throttled_tps =
                self.throttled_request_times.len() as f64 / self.trailing_window.as_secs_f64();

            if elapsed_seconds - self.last_time_point_added >= 0.03 {
                self.trailing_tps_data.push([elapsed_seconds, trailing_tps]);
                self.generated_tps_data
                    .push([elapsed_seconds, generated_tps]);
                self.target_tps_data
                    .push([elapsed_seconds, self.rate_limiter.target_rate]);
                self.throttled_tps_data
                    .push([elapsed_seconds, throttled_tps]);
                self.measured_tps_data
                    .push([elapsed_seconds, self.rate_limiter.request_rate]);

                self.last_time_point_added = elapsed_seconds;
            }

            // Print metrics to the terminal
            let accepted_tps = self.accepted_requests as f64 / elapsed_seconds;
            let total_tps = self.total_requests as f64 / elapsed_seconds;
            println!(
                "Elapsed: {:.2}s | Total TPS: {:.2} | Accepted TPS: {:.2} | Trailing TPS: {:.2} | Generated TPS: {:.2} | Target TPS: {:.2} | Throttled TPS: {:.2}",
                elapsed_seconds, total_tps, accepted_tps, trailing_tps, generated_tps, self.rate_limiter.target_rate, throttled_tps
            );

            ctx.request_repaint_after(Duration::from_millis(inter_request_delay));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            Plot::new("Rate Limiter Simulation")
                .view_aspect(2.0)
                .legend(egui_plot::Legend::default().position(Corner::LeftTop))
                .show(ui, |plot_ui| {
                    plot_ui.line(Line::new(self.trailing_tps_data.clone()).name("Trailing TPS"));
                    plot_ui.line(Line::new(self.generated_tps_data.clone()).name("Generated TPS"));
                    plot_ui.line(Line::new(self.target_tps_data.clone()).name("Target TPS"));
                    plot_ui.line(Line::new(self.throttled_tps_data.clone()).name("Throttled TPS"));
                    plot_ui.line(Line::new(self.measured_tps_data.clone()).name("Measured TPS"));
                });
        });
    }
}

#[derive(Clone)]
pub struct RequestGenerator {
    pub base_tps: f64,
    pub amplitudes: Vec<f64>,
    pub frequencies: Vec<f64>,
}

impl RequestGenerator {
    pub fn new(base_tps: f64, amplitudes: Vec<f64>, frequencies: Vec<f64>) -> Self {
        RequestGenerator {
            base_tps,
            amplitudes,
            frequencies,
        }
    }

    pub fn generate_request_rate(&self, elapsed_seconds: f64) -> f64 {
        let mut rate = self.base_tps;
        for (amplitude, frequency) in self.amplitudes.iter().zip(self.frequencies.iter()) {
            rate += amplitude * (2.0 * std::f64::consts::PI * frequency * elapsed_seconds).sin();
        }
        rate
    }
}
