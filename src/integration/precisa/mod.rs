# Changes outside src/integration/eureka_precisa/

## src/integration/mod.rs

```rust
pub mod eureka_precisa;
pub mod la_marzocco;

pub static KINDS: &[&Kind] = &[&la_marzocco::KIND, &eureka_precisa::KIND];

pub struct CliArgs {
    #[command(flatten)]
    la_marzocco: la_marzocco::config::Args,
    #[command(flatten)]
    eureka_precisa: eureka_precisa::config::Args,
}

impl CliArgs {
    pub fn configured(&self) -> Vec<(&'static Kind, Settings)> {
        [
            (&la_marzocco::KIND, self.la_marzocco.settings()),
            (&eureka_precisa::KIND, self.eureka_precisa.settings()),
        ]
        .into_iter()
        .filter_map(|(kind, settings)| Some((kind, settings?)))
        .collect()
    }
}
```

Also add `eureka_precisa` to the "Included:" line of the module docs.

## Cargo.toml

```toml
btleplug = "0.11"
uuid = "1"
futures = "0.3"   # if not there yet
```

btleplug talks to BlueZ over D-Bus and links `libdbus-1`. For the distroless
image either copy `libdbus-1.so.3` into it, or enable the `dbus` crate's
`vendored` feature:

```toml
dbus = { version = "0.9", features = ["vendored"] }
```

## Dockerfile / README

Bluetooth needs `--net=host` and `-v /run/dbus:/run/dbus:ro`; see this
integration's README.