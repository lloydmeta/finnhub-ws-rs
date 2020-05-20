## finnhub.io Websocket UI in Rust ![Continuous integration](https://github.com/lloydmeta/finnhub-ws-rs/workflows/Continuous%20integration/badge.svg?branch=master)

A [WASM](https://webassembly.org) app written in Rust using [Yew](https://yew.rs) that consumes the [finnhub.io](https://finnhub.io)
trades websocket API to list trades as they come in, with some basic colour coding. 

The working version is published at [beachape.com/finnhub-ws-rs](https://beachape.com/finnhub-ws-rs).

Areas explored:
- Compiling Rust to [Webassembly](https://webassembly.org)
- [finnhub.io](https://finnhub.io/docs/api#websocket-price) WS API 
- [Yew](https://yew.rs)
- [GH Actions](https://github.com/features/actions) for CI

## Building

### Install needed tools

```shell
cargo install wasm-pack
```

### Build

```shell
wasm-pack build --target web --out-name wasm --out-dir ./static
```

### Running locally

```shell
cargo +nightly install miniserve
```

```shell
miniserve ./static --index index.html
```