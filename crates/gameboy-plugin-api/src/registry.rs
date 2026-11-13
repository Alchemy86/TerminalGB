//! [`PluginRegistry`] — how a name in `plugins = …` becomes a running plugin.
//!
//! # Why a registry rather than a `match`
//!
//! The frontend used to name the shipped plugins directly: a `match` on the
//! configured string, one arm per plugin, in the terminal UI. That works
//! exactly until somebody else writes a plugin — there is no arm for it, and
//! adding one means editing the emulator.
//!
//! A registry is the smallest thing that fixes it. An embedder builds one,
//! adds whatever it likes, and hands it to the frontend:
//!
//! ```no_run
//! # use gameboy_plugin_api::{Capability, Plugin, PluginRegistry, PluginSpec};
//! # struct MyPlugin;
//! # impl Plugin for MyPlugin {
//! #     fn name(&self) -> &str { "my-plugin" }
//! #     fn capabilities(&self) -> Capability { Capability::OBSERVE }
//! #     fn as_any(&self) -> &dyn std::any::Any { self }
//! #     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
//! # }
//! # fn demo(mut registry: PluginRegistry) {
//! registry.add(PluginSpec::new(
//!     "my-plugin",
//!     "counts frames",
//!     |_cfg| Ok(Box::new(MyPlugin)),
//! ));
//! # }
//! ```
//!
//! **Nothing loads implicitly.** Being in the registry means "the user may ask
//! for this by name", not "this is running" — with the one documented
//! exception of [`PluginSpec::always`].

use crate::plugin::Plugin;
use std::collections::BTreeMap;

/// Everything a factory is given to build its plugin.
///
/// The values come from the frontend's configuration file and environment, so
/// a plugin's settings live beside the emulator's and a user edits one file.
/// Keys a plugin declares in [`PluginSpec::config_keys`] are recognised by the
/// config parser; anything else is reported as an unknown key rather than
/// silently ignored.
pub struct PluginConfig<'a> {
    /// The cartridge image. Several plugins scan it for tables at build time
    /// rather than reading a mapped bank later.
    pub rom: &'a [u8],
    values: BTreeMap<String, String>,
}

impl<'a> PluginConfig<'a> {
    pub fn new(rom: &'a [u8]) -> PluginConfig<'a> {
        PluginConfig {
            rom,
            values: BTreeMap::new(),
        }
    }

    /// Set one value. The frontend calls this for each configured key; the
    /// environment override, if any, is resolved before it gets here.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.values.insert(key.into(), value.into());
    }

    /// A configured value, or `None` if the user did not set it.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }
}

/// What the registry knows about one plugin.
pub struct PluginSpec {
    /// The name a user writes in `plugins = …`. Must match
    /// [`Plugin::name`](crate::Plugin::name).
    pub name: &'static str,
    /// One line for `--list-plugins` and the help overlay.
    pub summary: &'static str,
    /// Configuration keys this plugin reads. Declaring them is what stops the
    /// config parser calling them typos.
    pub config_keys: &'static [&'static str],
    /// Attach without being asked. **Use sparingly and only when the plugin
    /// costs nothing while idle** — the trainer view does this because it reads
    /// no memory at all until its panel is opened, and a user who has to
    /// configure a panel before they can press its key will never find it.
    pub always: bool,
    build: Box<dyn Fn(&PluginConfig) -> Result<Box<dyn Plugin>, String> + Send + Sync>,
}

impl PluginSpec {
    pub fn new(
        name: &'static str,
        summary: &'static str,
        build: impl Fn(&PluginConfig) -> Result<Box<dyn Plugin>, String> + Send + Sync + 'static,
    ) -> PluginSpec {
        PluginSpec {
            name,
            summary,
            config_keys: &[],
            always: false,
            build: Box::new(build),
        }
    }

    pub fn config_keys(mut self, keys: &'static [&'static str]) -> PluginSpec {
        self.config_keys = keys;
        self
    }

    /// See [`PluginSpec::always`] — read that first.
    pub fn always(mut self) -> PluginSpec {
        self.always = true;
        self
    }

    /// Build the plugin. `Err` is a message for the user, not a panic: a plugin
    /// that cannot start says why and the emulator runs without it.
    pub fn build(&self, cfg: &PluginConfig) -> Result<Box<dyn Plugin>, String> {
        (self.build)(cfg)
    }
}

/// The set of plugins this build knows how to construct.
#[derive(Default)]
pub struct PluginRegistry {
    specs: Vec<PluginSpec>,
}

impl PluginRegistry {
    pub fn new() -> PluginRegistry {
        PluginRegistry::default()
    }

    /// Add a plugin. A duplicate name replaces the earlier entry, so an
    /// embedder can substitute its own build of a shipped plugin.
    pub fn add(&mut self, spec: PluginSpec) -> &mut PluginRegistry {
        if let Some(slot) = self.specs.iter_mut().find(|s| s.name == spec.name) {
            *slot = spec;
        } else {
            self.specs.push(spec);
        }
        self
    }

    pub fn get(&self, name: &str) -> Option<&PluginSpec> {
        self.specs.iter().find(|s| s.name == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &PluginSpec> {
        self.specs.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Names, for an error message that lists what the user could have meant.
    pub fn names(&self) -> Vec<&'static str> {
        self.specs.iter().map(|s| s.name).collect()
    }

    /// Every configuration key any registered plugin reads — what the config
    /// parser must not call unknown.
    pub fn config_keys(&self) -> Vec<&'static str> {
        self.specs
            .iter()
            .flat_map(|s| s.config_keys.iter().copied())
            .collect()
    }

    /// The plugins that attach without being asked for.
    pub fn always_on(&self) -> impl Iterator<Item = &PluginSpec> {
        self.specs.iter().filter(|s| s.always)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capability, Plugin};

    struct Noop(&'static str);
    impl Plugin for Noop {
        fn name(&self) -> &str {
            self.0
        }
        fn capabilities(&self) -> Capability {
            Capability::NONE
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    #[test]
    fn a_registry_names_what_it_can_build_and_which_keys_are_not_typos() {
        let mut r = PluginRegistry::new();
        r.add(
            PluginSpec::new("alpha", "the first", |_| Ok(Box::new(Noop("alpha"))))
                .config_keys(&["alpha_file"]),
        );
        r.add(PluginSpec::new("beta", "the second", |_| Ok(Box::new(Noop("beta")))).always());

        assert_eq!(r.names(), vec!["alpha", "beta"]);
        assert_eq!(r.config_keys(), vec!["alpha_file"]);
        assert_eq!(r.always_on().map(|s| s.name).collect::<Vec<_>>(), ["beta"]);

        let rom = vec![0u8; 16];
        let mut cfg = PluginConfig::new(&rom);
        cfg.set("alpha_file", "/tmp/x");
        assert_eq!(cfg.get("alpha_file"), Some("/tmp/x"));
        assert_eq!(cfg.get("nope"), None);
        assert_eq!(r.get("alpha").unwrap().build(&cfg).unwrap().name(), "alpha");
        assert!(r.get("gamma").is_none());
    }

    /// An embedder may replace a shipped plugin with its own build under the
    /// same name — otherwise "install your own version" means forking.
    #[test]
    fn a_later_registration_replaces_an_earlier_one_of_the_same_name() {
        let mut r = PluginRegistry::new();
        r.add(PluginSpec::new("x", "stock", |_| Err("stock".into())));
        r.add(PluginSpec::new("x", "mine", |_| Ok(Box::new(Noop("x")))));
        let rom = Vec::new();
        assert_eq!(r.iter().count(), 1);
        assert!(r.get("x").unwrap().build(&PluginConfig::new(&rom)).is_ok());
    }
}
