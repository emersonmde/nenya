# Nenya

**Nenya** is an adaptive rate limiter using a Proportional-Integral-Derivative (PID) controller. This
project contains two major components:

- **Nenya**: A Rust crate for adaptive rate limiting.
- **Nenya-Sentinel**: A standalone rate limiter gRPC service that is intended to run as a sidecar
  for existing services.

## Overview

### Nenya

Nenya is a Rust crate that offers adaptive rate limiting functionality using a PID
controller. The crate aims to provide a dynamic and efficient way to manage
request rates, making it suitable for high-throughput services.

#### Features

- **PID Controller**: Utilizes a highly configurable Proportional-Integral-Derivative
  (PID) controller to dynamically adjust the rate limits based on current traffic
  patterns
- **Configurable Sliding Window**: Uses a configurable sliding window to
  determine Transactions Per Second (TPS), ensuring accurate rate limiting decisions
- **Configuration**: Allows fine-tuning of PID parameters (`kp`, `ki`, `kd`),
  error limits, output limits, and update intervals

### Nenya-Sentinel (Work In Progress)

Nenya-Sentinel is a standalone rate limiting service that will support gRPC for
easy integration as a sidecar in microservice architectures.

## Getting Started

### Running Examples

Nenya includes a simulation example for testing and tuning. You can
run the simulation with:

```sh
cargo run --example request_simulator_plot -- \
    --target_tps 80.0 \
    --min_tps 75.0 \
    --max_tps 100.0 \
    --trailing_window 1 \
    --duration 120 \
    --base_tps 80.0 \
    --amplitudes 20.0,7.0,10.0 \
    --frequencies 0.05,2.8,4.0 \
    --kp 0.8 \
    --ki 0.05 \
    --kd 0.04 \
    --error_limit 10.0 \
    --output_limit 3.0 \
    --update_interval 500 \
    --error_bias 0.0

```

Most of these arguments have sane defaults and can be omitted. For more details
see:

```sh
cargo run --example request_simulator_plot -- --help
```

## Adaptive Rate Limiting

The rate limiter achieves an adaptive rate limit using a
Proportional–Integral–Derivative (PID) controller which determines the target
rate limit based on the request rate. This implementation includes error
bias, accumulated error clamping, anti-windup feedback, and output clamping.

### Overview

1. **Error Calculation**: The error is calculated by subtracting the request
   rate from the setpoint.
2. **Proportional Term**: The proportional term is the product of the
   proportional gain and the error.
3. **Error Bias**:  The error is adjusted by a bias factor, reacting more to
   positive errors if $B > 0$ and more to negative errors if $B < 0$.
4. **Integral Term**: The integral term is the accumulated error over time,
   clamped to prevent windup.
5. **Derivative Term**: The derivative term is the rate of change of the error.
6. **Raw Correction**: The raw correction is the sum of the P, I, and D terms.
7. **Output Clamping**: The output is clamped to a specified limit to prevent
   excessive corrections.
8. **Anti-Windup Feedback**: If clamping occurs, the accumulated error is
   adjusted to prevent windup.
9. **Final Output**: The clamped correction is the final output of the PID
   controller.
10. **Request Limit Adjustment**: The clamped correction is added to the
    current request limit to derive the new request limit.

### 1. Error Calculation

The error $e(t)$ is calculated as the difference between the setpoint $S$ and
the request rate $r(t)$:

```math
e(t) = S - r(t)
```

### 2. Proportional Term (P)

The proportional term $P(t)$ is computed using the proportional gain $K_p$:

```math
P(t) = K_p \cdot e(t)
```

### 3. Error Bias

The error is adjusted by a bias $B$ to react more to positive or negative
errors:

```math
\text{biased\_error}(t) =
\begin{cases}
e(t) \cdot (1 + B) & \text{if } e(t) > 0 \\
e(t) \cdot (1 - B) & \text{if } e(t) \leq 0
\end{cases}
```

### 4. Integral Term (I)

The accumulated error $E(t)$ is clamped to prevent integral windup:

```math
E(t) = \text{clamp}\left( E(t-1) + \text{biased\_error}(t), -L, L \right)
```

where $L$ is the error limit.

The integral term $I(t)$ is then:

```math
I(t) = K_i \cdot E(t)
```

### 5. Derivative Term (D)

The derivative term $D(t)$ is computed using the derivative gain $K_d$
and the rate of change of the error:

```math
D(t) = K_d \cdot \frac{d e(t)}{dt}
```

For discrete time steps, this can be approximated as:

```math
D(t) = K_d \cdot \left( e(t) - e(t-1) \right)
```

### 6. Raw Correction

The raw correction $u(t)$ is the sum of the proportional, integral, and
derivative terms:

```math
u(t) = P(t) + I(t) + D(t)
```

### 7. Output Clamping

The output correction is clamped to prevent excessive output:

```math
u_{\text{clamped}}(t) = \text{clamp}(u(t), -M, M)
```

where $M$ is the output limit.

### 8. Anti-Windup Feedback

If the correction is clamped, the accumulated error $E(t)$ is adjusted to
prevent windup:

```math
\text{if } u(t) \neq u_{\text{clamped}}(t) \text{ then } E(t) = E(t) - \frac{u(t) - u_{\text{clamped}}(t)}{K_i}
```

### 9. Final Output

The final output of the PID controller is:

```math
u_{\text{clamped}}(t)
```

### 10. Request Limit Adjustment

The output is added to the current request limit $R(t-1)$ to derive the new
request limit $R(t)$:

```math
R(t) = R(t-1) + u_{\text{clamped}}(t)
```

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for more details.