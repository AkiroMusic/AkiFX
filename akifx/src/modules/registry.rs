//! Declarative module registry — the single source of truth for the module
//! set.
//!
//! [`define_modules!`] takes one table row per module and generates all the
//! pieces that historically had to be kept in sync by hand:
//!
//! 1. the umbrella `AkiFxParams` tree (`#[nested(id_prefix)]` fields),
//! 2. `create_default_chain()` (chain construction in table order),
//! 3. the static name/prefix tables and module count used by the GUI and
//!    tests,
//! 4. typed parameter access by module index.
//!
//! The table itself is invoked in `lib.rs`; adding a module = adding one row
//! there (plus its own file).

/// One row per module, in processing order:
/// `field, ModuleType, ParamsType, "id_prefix", "Display Name";`
#[macro_export]
macro_rules! define_modules {
    ( $( $field:ident, $mod_ty:ident, $params_ty:ident, $prefix:literal, $name:literal ; )* ) => {
        /// Umbrella parameter struct composing every module's params.
        ///
        /// Each module's params are nested with an `id_prefix` so their
        /// parameter IDs are namespaced. The `Arc` instances are shared
        /// between this tree (host automation/persistence) and the modules
        /// (DSP access).
        #[derive(nih_plug::prelude::Params)]
        pub struct AkiFxParams {
            $(
                #[nested(id_prefix = $prefix)]
                pub $field: std::sync::Arc<$params_ty>,
            )*

            /// Persisted module processing order. Initialized to identity.
            #[persist = "module_order"]
            pub module_order: parking_lot::Mutex<Vec<usize>>,

            /// Persisted UI zoom level. 1.0 = 100%.
            #[persist = "ui_zoom"]
            pub ui_zoom: parking_lot::Mutex<f32>,

            /// Persisted per-module enabled state (`true` = active), in chain
            /// order. Mirrors each module's bypass flag; applied to the live
            /// flags in `AkiFx::initialize()`. All-off on a fresh load.
            #[persist = "module_enabled"]
            pub module_enabled: parking_lot::Mutex<Vec<bool>>,
        }

        impl Default for AkiFxParams {
            fn default() -> Self {
                Self {
                    $(
                        $field: std::sync::Arc::new(<$params_ty as Default>::default()),
                    )*
                    module_order: parking_lot::Mutex::new((0..MODULE_COUNT).collect()),
                    ui_zoom: parking_lot::Mutex::new(1.0),
                    module_enabled: parking_lot::Mutex::new(vec![false; MODULE_COUNT]),
                }
            }
        }

        /// Build the default processing chain, in registry order. Every module
        /// starts bypassed.
        pub fn create_default_chain(params: &AkiFxParams) -> $crate::ModuleChain {
            let mut chain = $crate::ModuleChain::new();
            $(
                let bypass = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
                chain.push(Box::new(<$mod_ty>::new(params.$field.clone(), bypass)));
            )*
            chain
        }

        /// Human-readable module display names, in registry order.
        pub const MODULE_NAMES: &[&str] = &[ $( $name, )* ];

        /// Module `id_prefix`es (as used in the parameter tree), in registry
        /// order.
        pub const MODULE_PREFIXES: &[&str] = &[ $( $prefix, )* ];

        /// Number of registered modules.
        pub const MODULE_COUNT: usize = MODULE_NAMES.len();

        /// Clone the params of module `idx` as a trait object (for the GUI
        /// entries), or `None` when out of range.
        pub fn params_for_module(
            params: &AkiFxParams,
            idx: usize,
        ) -> Option<std::sync::Arc<dyn nih_plug::prelude::Params>> {
            const GETTERS: &[fn(&AkiFxParams) -> std::sync::Arc<dyn nih_plug::prelude::Params>] =
                &[ $( |p| p.$field.clone(), )* ];
            GETTERS.get(idx).map(|get| get(params))
        }
    };
}
