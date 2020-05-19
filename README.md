

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