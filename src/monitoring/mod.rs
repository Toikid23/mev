pub mod logging;
pub mod metrics;

/// Retourne une chaîne de caractères statique représentant le type de DEX
/// pour un objet Pool donné. Utile pour les labels Prometheus.
pub fn get_pool_type_name(pool: &crate::decoders::Pool) -> &'static str {
    match pool {
        crate::decoders::Pool::RaydiumAmmV4(_) => "RaydiumAmmV4",
        crate::decoders::Pool::RaydiumCpmm(_) => "RaydiumCpmm",
        crate::decoders::Pool::RaydiumClmm(_) => "RaydiumClmm",
        crate::decoders::Pool::MeteoraDammV1(_) => "MeteoraDammV1",
        crate::decoders::Pool::MeteoraDammV2(_) => "MeteoraDammV2",
        crate::decoders::Pool::MeteoraDlmm(_) => "MeteoraDlmm",
        crate::decoders::Pool::OrcaWhirlpool(_) => "OrcaWhirlpool",
        crate::decoders::Pool::PumpAmm(_) => "PumpAmm",
    }
}