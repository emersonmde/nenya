# ⚠️ DEPRECATED: nenya-sentinel

**This crate has been merged into [nenya](https://crates.io/crates/nenya).**

## Migration

The `nenya-sentinel` binary has been merged into the main `nenya` crate as a single unified package.

### Uninstall the old binary:
```bash
cargo uninstall nenya-sentinel
```

### Install the new unified binary:
```bash
cargo install nenya
```

### Run the binary:
```bash
# The binary is now called 'nenya' (not 'nenya-sentinel')
nenya --help
```

## What Changed?

- **Old**: Two separate crates (`nenya` library + `nenya-sentinel` binary)
- **New**: Single `nenya` crate with both library and binary

All functionality from `nenya-sentinel` is now available in the `nenya` binary.

## Documentation

See the [main repository](https://github.com/emersonmde/nenya) for full documentation.
