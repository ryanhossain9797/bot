FROM rustlang/rust:nightly AS builder

RUN apt-get update

RUN apt-get install -y \
    pkg-config \
    libssl-dev \
    libclang-dev \
    cmake \
    build-essential \
    git \
    libvulkan-dev \
    glslc \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-gnu

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY chatbot/src ./chatbot/src
COPY chatbot/Cargo.toml ./chatbot/
COPY re_framework ./re_framework
COPY framework ./framework
COPY probe ./probe
COPY extractor ./extractor
COPY .git ./.git

RUN cargo build --release --package chatbot

RUN (git rev-parse --short HEAD && echo && git log -1 --pretty=%B) > /app/commit.txt || echo "unknown" > /app/commit.txt

FROM zireael9797/bot-base:latest


COPY --from=builder /app/target/release/chatbot /app/chatbot

COPY --from=builder /app/commit.txt /app/commit.txt

COPY chatbot/Dockerfile.worker /app/worker/Dockerfile


CMD ["/app/chatbot"]