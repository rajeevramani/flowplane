use argon2::{Algorithm, Argon2, Params, Version};

pub fn password_hasher() -> Argon2<'static> {
    // Tuned for interactive API calls: Argon2id with moderate memory and a single iteration
    // keeps verification under 10ms on development hardware while retaining side-channel
    // protections.
    const MEMORY_COST_KIB: u32 = 768; // 0.75 MiB keeps verification below the latency budget
    const ITERATIONS: u32 = 1;
    const PARALLELISM: u32 = 1;
    let params = Params::new(MEMORY_COST_KIB, ITERATIONS, PARALLELISM, Some(32))
        .expect("valid Argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}
