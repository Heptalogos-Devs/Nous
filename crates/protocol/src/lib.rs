//! Generated transport contracts; cognitive domain owners stay independent.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[allow(
    clippy::all,
    clippy::too_many_lines,
    clippy::large_futures,
    clippy::excessive_nesting,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::large_enum_variant,
    clippy::fn_params_excessive_bools,
    clippy::empty_docs
)]
pub mod nous {
    pub mod wave {
        pub mod v1alpha1 {
            include!("generated/nous/wave/v1alpha1/nous.wave.v1alpha1.rs");
        }
        pub mod kernel {
            pub mod v1alpha1 {
                include!("generated/nous/wave/kernel/v1alpha1/nous.wave.kernel.v1alpha1.rs");
            }
        }
    }
}
#[allow(
    clippy::all,
    clippy::too_many_lines,
    clippy::large_futures,
    clippy::excessive_nesting,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::large_enum_variant,
    clippy::fn_params_excessive_bools
)]
pub mod grpc {
    pub mod health {
        pub mod v1 {
            include!("generated/grpc/health/v1/grpc.health.v1.rs");
        }
    }
}
pub use nous::wave::kernel::v1alpha1 as kernel;
pub use nous::wave::v1alpha1 as public;

pub mod google {
    pub mod rpc {
        include!("generated/google/rpc/google.rpc.rs");
    }
}
