# Stage 1: Build frontend
FROM node:20-alpine AS frontend-build
WORKDIR /app/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# Stage 2: Build Rust backend
FROM rust:1.83-alpine AS backend-build
RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static pkgconfig
WORKDIR /app/backend-rs
COPY backend-rs/ ./
ENV OPENSSL_STATIC=1
RUN cargo build --release -p asspp-standalone

# Stage 3: Runtime (~15MB final image)
FROM alpine:3.21
RUN apk add --no-cache ca-certificates
WORKDIR /app
COPY --from=backend-build /app/backend-rs/target/release/asspp-standalone ./asspp-standalone
COPY --from=frontend-build /app/frontend/dist ./public
RUN mkdir -p /data/packages
EXPOSE 8080
ENV DATA_DIR=/data PORT=8080 PUBLIC_DIR=/app/public
CMD ["./asspp-standalone"]
