use std::collections::VecDeque;
use std::io::{stdout, Write};
use std::thread;
use std::time::{Duration, Instant};

use clap::{Arg, Command};

use nenya::pid_controller::PIDController;
use nenya::RateLimiter;

const LINE_LENGTH: usize = 80;

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
    let mut rate_limiter = RateLimiter::new(
        target_tps,
        min_tps,
        max_tps,
        pid_controller,
        update_interval,
    );

    let generator = RequestGenerator::new(base_tps, amplitudes, frequencies);
    generate_requests(&mut rate_limiter, &generator, trailing_window, duration);
}

fn generate_requests(
    rate_limiter: &mut RateLimiter,
    generator: &RequestGenerator,
    trailing_window: Duration,
    duration: Duration,
) {
    let start = Instant::now();
    let mut accepted_requests = 0;
    let mut total_requests = 0;
    let mut total_tps = 0.0;
    let mut accepted_tps = 0.0;
    let mut trailing_tps = 0.0;

    print!("\x1B[2J");
    print!("\x1B[0;0H");
    println!("Rate Limiter Target");
    println!("-------------------");

    let mut output_buffer = vec![' '; LINE_LENGTH];
    let mut request_times = VecDeque::new();

    while Instant::now().duration_since(start) < duration {
        let elapsed_seconds = Instant::now().duration_since(start).as_secs_f64();

        // Generate a varying request rate using the RequestGenerator
        let generated_tps = generator.generate_request_rate(elapsed_seconds);
        let inter_request_delay = if generated_tps > 0.0 {
            (1000.0 / generated_tps) as u64
        } else {
            1000
        };

        let should_accept_request = rate_limiter.should_throttle();
        total_requests += 1;
        let now = Instant::now();

        // Shift all characters in the buffer to the left
        for i in 1..LINE_LENGTH {
            output_buffer[i - 1] = output_buffer[i];
        }

        // Add new indicator at the end of the buffer
        if should_accept_request {
            accepted_requests += 1;
            output_buffer[LINE_LENGTH - 1] = '.';
            request_times.push_back(now);
        } else {
            output_buffer[LINE_LENGTH - 1] = '!';
        }

        // Remove old timestamps outside the trailing window
        while let Some(&time) = request_times.front() {
            if now.duration_since(time) > trailing_window {
                request_times.pop_front();
            } else {
                break;
            }
        }

        trailing_tps = request_times.len() as f64 / trailing_window.as_secs_f64();

        // Save cursor
        print!("\x1B7");
        // Clear screen
        print!("\x1B[0J");
        print!("\r[{}]\n", output_buffer.iter().collect::<String>());

        let elapsed = Instant::now().duration_since(start).as_secs_f64();
        accepted_tps = accepted_requests as f64 / elapsed;
        total_tps = total_requests as f64 / elapsed;
        print_metrics(
            &total_tps,
            &accepted_tps,
            &trailing_tps,
            rate_limiter,
            generated_tps,
        );
        println!();

        // Restore cursor position
        print!("\x1B8");
        stdout().flush().unwrap();
        thread::sleep(Duration::from_millis(inter_request_delay));
    }
    let elapsed = Instant::now().duration_since(start).as_secs_f64();

    print!("\x1B[4;0H");
    print_metrics(&total_tps, &accepted_tps, &trailing_tps, rate_limiter, 0.0);
    println!("\rElapsed Time (s): {:.2}", elapsed);
    println!("\rAccepted Requests: {}", accepted_requests);
}

fn print_metrics(
    total_tps: &f64,
    accepted_tps: &f64,
    trailing_tps: &f64,
    rate_limiter: &RateLimiter,
    generated_tps: f64,
) {
    println!("\rTotal TPS: {:.2}", total_tps);
    println!("\rAccepted TPS: {:.2}", accepted_tps);
    println!("\rTrailing Accepted TPS: {:.2}", trailing_tps);
    println!("\rGenerated TPS: {:.2}", generated_tps);
    println!("\rTarget TPS: {:.2}", rate_limiter.target_rate());
    println!("\rMeasured TPS: {:.2}", rate_limiter.request_rate());
}

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
