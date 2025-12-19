# 1. 构建阶段
FROM rust:1.85 as builder

WORKDIR /app

# 安装构建依赖
# 引入 clang 以解决 ARM64 下 whisper.cpp 的编译问题
RUN apt-get update && \
    apt-get install -y libopus-dev pkg-config libsqlite3-dev curl libclang-dev llvm-dev cmake clang && \
    rm -rf /var/lib/apt/lists/*

COPY . .

# 强制使用 Clang 编译器
ENV CC=clang
ENV CXX=clang++

# 编译项目
RUN cargo build --release

# 下载模型文件并进行校验
# 修正：去掉了 URL 中的 /models 路径，直接指向根目录
RUN curl -L -f -o ggml-base.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin && \
    # 校验文件大小，确保没有下载到 1KB 的 LFS 指针文件
    if [ $(stat -c%s "ggml-base.bin") -lt 104857600 ]; then \
        echo "Error: Downloaded file is too small. It might be a Git LFS pointer." && exit 1; \
    else \
        echo "Model downloaded successfully."; \
    fi

# 2. 运行阶段
FROM debian:bookworm-slim

WORKDIR /app

# 安装运行时依赖
# libstdc++6: C++ 标准库
# libgomp1: OpenMP 支持
RUN apt-get update && \
    apt-get install -y libopus0 libsqlite3-0 ca-certificates curl libstdc++6 libgomp1 && \
    rm -rf /var/lib/apt/lists/*

# 复制编译产物
COPY --from=builder /app/target/release/heart_mirror_brain /app/heart_mirror_brain
# 复制模型文件
COPY --from=builder /app/ggml-base.bin /app/ggml-base.bin
# 复制数据库
COPY history-emotion.db ./

# 暴露端口
EXPOSE 4321

# 启动命令
CMD ["./heart_mirror_brain"]
