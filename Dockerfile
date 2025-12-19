FROM rust:latest as builder

WORKDIR /app

RUN apt-get update && \
    apt-get install -y libopus-dev pkg-config libsqlite3-dev curl libclang-dev llvm-dev cmake && \
    rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt

COPY . .

RUN cargo build --release

RUN curl -L -o ggml-base.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/models/ggml-base.bin && \
    curl -L -o ggml-base.en.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/models/ggml-base.en.bin

FROM debian:sid-slim

WORKDIR /app

RUN apt-get update && \
    apt-get install -y libopus0 libsqlite3-0 ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/heart_mirror_brain /app/heart_mirror_brain
COPY --from=builder /app/ggml-base.bin /app/ggml-base.bin
COPY --from=builder /app/ggml-base.en.bin /app/ggml-base.en.bin
COPY history-emotion.db ./

EXPOSE 4321

CMD ["./heart_mirror_brain"]
