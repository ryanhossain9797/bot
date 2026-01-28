# Build stage
# Manual cross-compilation to x86_64 (robust against Docker-in-Docker issues)
FROM rustlang/rust:nightly-bookworm AS builder

# Update package list
RUN apt-get update

# Install build dependencies and cross-compilation toolchain
RUN apt-get install -y \
    pkg-config \
    libssl-dev \
    libclang-dev \
    cmake \
    build-essential \
    git \
    gcc-x86-64-linux-gnu \
    g++-x86-64-linux-gnu \
    libvulkan-dev \
    glslc \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Add x86_64 target
RUN rustup target add x86_64-unknown-linux-gnu

# Set working directory
WORKDIR /app

# Copy workspace files (build context should be parent directory)
COPY Cargo.toml Cargo.lock ./
COPY framework ./framework
COPY chatbot ./chatbot
COPY .git ./.git

# Configure environment for cross-compilation
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++ \
    AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar

# Build the project in release mode for x86_64
RUN cargo build --release --target x86_64-unknown-linux-gnu --package chatbot

# Capture git commit message
RUN git log -1 --pretty=%B > /app/commit.txt || echo "unknown" > /app/commit.txt

# Runtime stage - uses base image from Docker Hub
# Explicitly use linux/amd64 platform to ensure x86_64 final image
# Replace 'zireael9797' with your actual Docker Hub username
FROM zireael9797/bot-base:latest

# Copy binary from builder (only this changes on code updates)
# cross builds to target/x86_64-unknown-linux-gnu/release
COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/chatbot /app/chatbot

# Copy commit message file
COPY --from=builder /app/commit.txt /app/commit.txt

# Note: configuration.rs must be provided at build time
# The DISCORD_TOKEN is read from configuration::client_tokens::DISCORD_TOKEN at compile time
# Make sure src/configuration.rs exists with your token before building

# Run the bot
CMD ["/app/chatbot"]