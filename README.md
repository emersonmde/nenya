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

In technical documentation, the idiomatic way to present this PID controller would involve a combination of mathematical
notation and clear, structured descriptions of each step. Here’s how you can structure the documentation:

---

## PID Controller

The rate limiter achieves an adapative rate limit using a Proportional–integral–derivative controller which determines
the target rate limit based on the request rate. This implementation includes with error bias,
accumulated error clamping, anti-windup feedback, and output clamping.

Certainly! Here's the updated documentation reflecting the rate-limiting context:

---

## PID Controller with Error Bias, Accumulated Error Clamping, Anti-Windup Feedback, Output Clamping, and Request Limit Adjustment

### 1. Error Calculation

The error $e(t)$ is calculated as the difference between the setpoint $S$ and the request rate $r(t)$:

$e(t) = S - r(t)$

### 2. Proportional Term (P)

The proportional term $P(t)$ is computed using the proportional gain $K_p$:

$P(t) = K_p \cdot e(t)$

### 3. Error Bias

The error is adjusted by a bias $B$ depending on its sign:

```math
\text{biased\_error}(t) =
\begin{cases}
e(t) \cdot |B| & \text{if } e(t) > 0 \\
e(t) / |B| & \text{if } e(t) \leq 0
\end{cases}
```

### 4. Integral Term (I)

The accumulated error $E(t)$ is clamped to prevent integral windup:

```math
E(t) = \text{clamp}\left( E(t-1) + \text{biased\_error}(t), -L, L \right)
```

where $L$ is the error limit.

The integral term $I(t)$ is then:

$I(t) = K_i \cdot E(t)$

### 5. Derivative Term (D)

The derivative term $D(t)$ is computed using the derivative gain $K_d$ and the rate of change of the error:

$D(t) = K_d \cdot \frac{d e(t)}{dt}$

For discrete time steps, this can be approximated as:

$D(t) = K_d \cdot \left( e(t) - e(t-1) \right)$

### 6. Raw Correction

The raw correction $u(t)$ is the sum of the proportional, integral, and derivative terms:

$u(t) = P(t) + I(t) + D(t)$

### 7. Output Clamping

The output correction is clamped to prevent excessive output:

$u_{\text{clamped}}(t) = \text{clamp}(u(t), -M, M)$

where $M$ is the output limit.

### 8. Anti-Windup Feedback

If the correction is clamped, the accumulated error $E(t)$ is adjusted to prevent windup:

$\text{if } u(t) \neq u_{\text{clamped}}(t) \text{ then } E(t) = E(t) - \frac{u(t) - u_{\text{clamped}}(t)}{K_i}$

### 9. Final Output

The final output of the PID controller is:

$u_{\text{clamped}}(t)$

### 10. Request Limit Adjustment

The output is added to the current request limit $R(t-1)$ to derive the new request limit $R(t)$:

$R(t) = R(t-1) + u_{\text{clamped}}(t)$

---

### Explanation

1. **Error Calculation**: The error is calculated by subtracting the request rate from the setpoint.
2. **Proportional Term**: The proportional term is the product of the proportional gain and the error.
3. **Error Bias**: The error is adjusted by a bias factor depending on whether it is positive or negative.
4. **Integral Term**: The integral term is the accumulated error over time, clamped to prevent windup.
5. **Derivative Term**: The derivative term is the rate of change of the error.
6. **Raw Correction**: The raw correction is the sum of the P, I, and D terms.
7. **Output Clamping**: The output is clamped to a specified limit to prevent excessive corrections.
8. **Anti-Windup Feedback**: If clamping occurs, the accumulated error is adjusted to prevent windup.
9. **Final Output**: The clamped correction is the final output of the PID controller.
10. **Request Limit Adjustment**: The clamped correction is added to the current request limit to derive the new request
    limit.

This structured and detailed explanation ensures that the documentation is clear, comprehensive, and accessible to other
engineers in a rate-limiting context.

---

### Explanation

1. **Error Calculation**: The error is calculated by subtracting the process variable from the setpoint.
2. **Proportional Term**: The proportional term is the product of the proportional gain and the error.
3. **Error Bias**: The error is adjusted by a bias factor depending on whether it is positive or negative.
4. **Integral Term**: The integral term is the accumulated error over time, clamped to prevent windup.
5. **Derivative Term**: The derivative term is the rate of change of the error.
6. **Raw Correction**: The raw correction is the sum of the P, I, and D terms.
7. **Output Clamping**: The output is clamped to a specified limit to prevent excessive corrections.
8. **Anti-Windup Feedback**: If clamping occurs, the accumulated error is adjusted to prevent windup.
9. **Final Output**: The clamped correction is the final output of the PID controller.

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for more details.