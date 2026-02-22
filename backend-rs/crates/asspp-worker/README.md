# asspp-worker

Cloudflare Workers deployment for AssppWeb.

## Prerequisites

- [Rust](https://rustup.rs/) with `wasm32-unknown-unknown` target
- [Node.js](https://nodejs.org/) (for frontend build and wrangler)
- [wrangler](https://developers.cloudflare.com/workers/wrangler/) CLI (`npm i -g wrangler`)

```bash
rustup target add wasm32-unknown-unknown
```

## Setup

### 1. Create Cloudflare resources

```bash
# Login to Cloudflare
wrangler login

# Create R2 bucket
wrangler r2 bucket create asspp-ipa

# Create KV namespace
wrangler kv namespace create TASK_KV
# Note the namespace ID from the output
```

### 2. Configure wrangler.toml

```bash
cp wrangler.example.toml wrangler.toml
```

Edit `wrangler.toml` and fill in:

- `[[kv_namespaces]]` → `id` = your KV namespace ID from step 1
- `[vars]` → `PUBLIC_BASE_URL` = your deployment URL (e.g. `https://asspp.example.com`)

### 3. Install frontend dependencies

```bash
cd ../../../frontend && npm install
```

## Development

```bash
wrangler dev
```

This will build the frontend, compile the Rust worker to WASM, and start a local dev server.

## Deploy

```bash
wrangler deploy
```
