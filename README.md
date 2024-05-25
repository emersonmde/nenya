# Nenya

**Nenya** is a work-in-progress project designed to provide a robust and
flexible rate limiter using a PID controller. Nenya is organized as a
Cargo workspace containing two main components:

- **Nenya**: A Rust crate for rate limiting using a PID controller.
- **Nenya-Sentinel**: A standalone rate limiter that will eventually be
  developed into a gRPC service intended to run as a sidecar for existing services.

## Overview

### Nenya

Nenya is a Rust crate that offers rate limiting functionality powered by a PID
controller. The crate aims to provide a dynamic and efficient way to manage
request rates, making it suitable for high-throughput services.

#### Features

- **PID Controller**: Utilizes a Proportional-Integral-Derivative (PID)
  controller to dynamically adjust the rate limits based on current traffic patterns.
- **Configurable Sliding Window**: Uses a configurable sliding window to
  determine Transactions Per Second (TPS), ensuring accurate and adaptive rate limiting.
- **Configuration**: Allows fine-tuning of PID parameters (`kp`, `ki`, `kd`),
  error limits, output limits, and update intervals.

### Nenya-Sentinel

Nenya-Sentinel will be a standalone rate limiting service that will
support gRPC for easy integration as a sidecar in microservice
architectures. Although development has not yet started, it is included in
the workspace as part of this project's roadmap.

## Getting Started

### Running Examples

Currently, Nenya includes a simulation example for testing and tuning. You can
run the simulation with:

```sh
cargo run --example request_simulator -- \
    --base_tps 80.0 \
    --min_tps 1.0 \
    --max_tps 60.0 \
    --target_tps 40.0 \
    --trailing_window 5 \
    --duration 120 \
    --amplitudes 40.0,10.0 \
    --frequencies 0.1,0.5 \
    --kp 0.5 \
    --ki 0.1 \
    --kd 0.05 \
    --error_limit 100.0 \
    --output_limit 5.0 \
    --update_interval 1000
```

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for more details.