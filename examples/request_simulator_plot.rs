use std::collections::VecDeque;
use std::time::{Duration, Instant};

use clap::{Arg, Command};
use eframe::egui;
use egui::ViewportBuilder;
use egui_plot::{Corner, Line, Plot};

use nenya::pid_controller::PIDControllerBuilder;
use nenya::{RateLimiter, RateLimiterBuilder};

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
                .value_parser(clap::value_parser!(f32))
                .default_value("1.0")
                .help("Lower bound of TPS for the rate limiter"),
        )
        .arg(
            Arg::new("max_tps")
                .short('x')
                .long("max_tps")
                .value_parser(clap::value_parser!(f32))
                .default_value("60.0")
                .help("Upper bound of TPS for the rate limiter"),
        )
        .arg(
            Arg::new("target_tps")
                .short('t')
                .long("target_tps")
                .value_parser(clap::value_parser!(f32))
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
                .value_parser(clap::value_parser!(f32))
                .default_value("0.8")
                .help("Proportional gain for the PID controller"),
        )
        .arg(
            Arg::new("ki")
                .long("ki")
                .value_parser(clap::value_parser!(f32))
                .default_value("0.05")
                .help("Integral gain for the PID controller"),
        )
        .arg(
            Arg::new("kd")
                .long("kd")
                .value_parser(clap::value_parser!(f32))
                .default_value("0.04")
                .help("Derivative gain for the PID controller"),
        )
        .arg(
            Arg::new("error_bias")
                .long("error_bias")
                .value_parser(clap::value_parser!(f32))
                .default_value("1.0")
                .help("Bias factor for the integral term"),
        )
        .arg(
            Arg::new("error_limit")
                .long("error_limit")
                .value_parser(clap::value_parser!(f32))
                .help("Error limit for the PID controller (defaults to 0.2 * target_tps)"),
        )
        .arg(
            Arg::new("output_limit")
                .long("output_limit")
                .value_parser(clap::value_parser!(f32))
                .help("Output limit for the PID controller (defaults to 0.05 * target_tps)"),
        )
        .arg(
            Arg::new("update_interval")
                .long("update_interval")
                .value_parser(clap::value_parser!(u64))
                .default_value("500")
                .help("Update interval for the PID controller (milliseconds)"),
        )
        .get_matches();

    let base_tps = *matches.get_one::<f64>("base_tps").unwrap();
    let target_tps = *matches.get_one::<f32>("target_tps").unwrap();
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

    let min_tps = *matches.get_one::<f32>("min_tps").unwrap();
    let max_tps = *matches.get_one::<f32>("max_tps").unwrap();
    let kp = *matches.get_one::<f32>("kp").unwrap();
    let ki = *matches.get_one::<f32>("ki").unwrap();
    let kd = *matches.get_one::<f32>("kd").unwrap();
    let error_bias = *matches.get_one::<f32>("error_bias").unwrap();
    let error_limit = matches.get_one::<f32>("error_limit").copied();
    let output_limit = matches.get_one::<f32>("output_limit").copied();
    let update_interval =
        Duration::from_millis(*matches.get_one::<u64>("update_interval").unwrap());

    let mut builder = PIDControllerBuilder::new(target_tps)
        .kp(kp)
        .ki(ki)
        .kd(kd)
        .error_bias(error_bias);

    // Apply error_limit (default: 0.2 * target_tps)
    let error_limit = error_limit.unwrap_or(0.2 * target_tps);
    builder = builder.error_limit(error_limit);

    // Apply output_limit (default: 0.05 * target_tps)
    let output_limit = output_limit.unwrap_or(0.05 * target_tps);
    builder = builder.output_limit(output_limit);

    let pid_controller = builder.build();
    let rate_limiter = RateLimiterBuilder::new(target_tps)
        .min_rate(min_tps)
        .max_rate(max_tps)
        .pid_controller(pid_controller)
        .update_interval(update_interval)
        .build();

    let generator = RequestGenerator::new(base_tps, amplitudes, frequencies);

    let trailing_window_clone: &'static mut Duration = Box::leak(Box::new(trailing_window));
    let duration_clone: &'static mut Duration = Box::leak(Box::new(duration));
    eframe::run_native(
        "Rate Limiter Simulation",
        eframe::NativeOptions {
            viewport: ViewportBuilder::default().with_maximized(true),
            centered: true,
            ..Default::default()
        },
        Box::new(|_cc| {
            Ok(Box::new(App::new(
                rate_limiter,
                generator,
                *trailing_window_clone,
                *duration_clone,
            )))
        }),
    )
    .unwrap();
}

struct App {
    rate_limiter: RateLimiter<f32>,
    generator: RequestGenerator,
    trailing_window: Duration,
    duration: Duration,
    start: Instant,
    last_frame: Instant,
    accepted_requests: usize,
    total_requests: usize,
    setpoint_data: Vec<[f64; 2]>,
    trailing_tps_data: Vec<[f64; 2]>,
    generated_tps_data: Vec<[f64; 2]>,
    target_tps_data: Vec<[f64; 2]>,
    throttled_tps_data: Vec<[f64; 2]>,
    trailing_generated_tps_data: Vec<[f64; 2]>,
    accepted_request_times: VecDeque<Instant>,
    throttled_request_times: VecDeque<Instant>,
    all_request_times: VecDeque<Instant>,
    last_time_point_added: f64,
    request_accumulator: f64,
}

impl App {
    fn new(
        rate_limiter: RateLimiter<f32>,
        generator: RequestGenerator,
        trailing_window: Duration,
        duration: Duration,
    ) -> Self {
        let now = Instant::now();
        Self {
            rate_limiter,
            generator,
            trailing_window,
            duration,
            start: now,
            last_frame: now,
            accepted_requests: 0,
            total_requests: 0,
            setpoint_data: Vec::new(),
            trailing_tps_data: Vec::new(),
            generated_tps_data: Vec::new(),
            target_tps_data: Vec::new(),
            throttled_tps_data: Vec::new(),
            trailing_generated_tps_data: Vec::new(),
            accepted_request_times: VecDeque::new(),
            throttled_request_times: VecDeque::new(),
            all_request_times: VecDeque::new(),
            last_time_point_added: 0.0,
            request_accumulator: 0.0,
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let elapsed_seconds = self.start.elapsed().as_secs_f64();

        if elapsed_seconds < self.duration.as_secs_f64() {
            // Generate a varying request rate using the RequestGenerator
            let generated_tps = self.generator.generate_request_rate(elapsed_seconds);

            // Calculate actual time elapsed since last frame
            let frame_duration = now.duration_since(self.last_frame).as_secs_f64();
            self.last_frame = now;

            // Accumulate fractional requests to avoid losing precision at low rates
            self.request_accumulator += generated_tps * frame_duration;

            // Generate requests from accumulated fractional count
            let requests_this_frame = self.request_accumulator.floor() as usize;
            self.request_accumulator -= requests_this_frame as f64;

            // Process multiple requests per frame to match the generated rate
            for _ in 0..requests_this_frame {
                let should_throttle_request = self.rate_limiter.should_throttle();
                self.total_requests += 1;
                let now = Instant::now();

                // Track all requests for trailing generated TPS
                self.all_request_times.push_back(now);

                // Add new indicator at the end of the buffer
                if should_throttle_request {
                    self.throttled_request_times.push_back(now);
                } else {
                    self.accepted_requests += 1;
                    self.accepted_request_times.push_back(now);
                }
            }

            let now = Instant::now();

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

            while let Some(&time) = self.all_request_times.front() {
                if now.duration_since(time) > self.trailing_window {
                    self.all_request_times.pop_front();
                } else {
                    break;
                }
            }

            let trailing_tps =
                self.accepted_request_times.len() as f64 / self.trailing_window.as_secs_f64();
            let throttled_tps =
                self.throttled_request_times.len() as f64 / self.trailing_window.as_secs_f64();
            let trailing_generated_tps =
                self.all_request_times.len() as f64 / self.trailing_window.as_secs_f64();

            if elapsed_seconds - self.last_time_point_added >= 0.033 {
                self.setpoint_data
                    .push([elapsed_seconds, self.rate_limiter.setpoint() as f64]);
                self.trailing_tps_data.push([elapsed_seconds, trailing_tps]);
                self.generated_tps_data
                    .push([elapsed_seconds, generated_tps]);
                self.target_tps_data
                    .push([elapsed_seconds, self.rate_limiter.target_rate() as f64]);
                self.throttled_tps_data
                    .push([elapsed_seconds, throttled_tps]);
                self.trailing_generated_tps_data
                    .push([elapsed_seconds, trailing_generated_tps]);

                self.last_time_point_added = elapsed_seconds;
            }

            // Fixed frame rate
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            Plot::new("Rate Limiter Simulation")
                .view_aspect(2.0)
                .legend(egui_plot::Legend::default().position(Corner::LeftTop))
                .show(ui, |plot_ui| {
                    plot_ui.line(Line::new(self.setpoint_data.clone()).name("Setpoint"));
                    plot_ui.line(
                        Line::new(self.generated_tps_data.clone())
                            .name("Generated TPS (instantaneous)"),
                    );
                    plot_ui.line(
                        Line::new(self.trailing_generated_tps_data.clone())
                            .name("Trailing Generated TPS"),
                    );
                    plot_ui.line(
                        Line::new(self.trailing_tps_data.clone()).name("Trailing Accepted TPS"),
                    );
                    plot_ui.line(
                        Line::new(self.throttled_tps_data.clone()).name("Trailing Throttled TPS"),
                    );
                    plot_ui.line(
                        Line::new(self.target_tps_data.clone()).name("Rate Limit Target TPS"),
                    );
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
