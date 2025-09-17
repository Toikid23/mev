// DANS : src/execution/routing.rs (VERSION FINALE ET COMPLÈTE)

use crate::state::validator_intel::ValidatorIntel;
use lazy_static::lazy_static;
use std::collections::HashMap;

// On ajoute toutes les régions connues
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JitoRegion {
    Amsterdam,
    Dublin,
    Frankfurt,
    London,
    NewYork,
    SaltLakeCity,
    Singapore,
    Tokyo,
}

// Struct pour retourner un résultat de routage complet
#[derive(Debug, Clone)]
pub struct RoutingInfo {
    pub region: JitoRegion,
    pub estimated_latency_ms: u32,
}

lazy_static! {
    // --- SOURCE DE VÉRITÉ POUR LES ENDPOINTS JITO ---
    pub static ref JITO_RPC_ENDPOINTS: HashMap<JitoRegion, &'static str> = {
        let mut m = HashMap::new();
        m.insert(JitoRegion::Amsterdam, "https://amsterdam.mainnet.block-engine.jito.wtf/api/v1/bundles");
        m.insert(JitoRegion::Dublin, "https://dublin.mainnet.block-engine.jito.wtf/api/v1/bundles");
        m.insert(JitoRegion::Frankfurt, "https://frankfurt.mainnet.block-engine.jito.wtf/api/v1/bundles");
        m.insert(JitoRegion::London, "https://london.mainnet.block-engine.jito.wtf/api/v1/bundles");
        m.insert(JitoRegion::NewYork, "https://ny.mainnet.block-engine.jito.wtf/api/v1/bundles");
        m.insert(JitoRegion::SaltLakeCity, "https://slc.mainnet.block-engine.jito.wtf/api/v1/bundles");
        m.insert(JitoRegion::Singapore, "https://singapore.mainnet.block-engine.jito.wtf/api/v1/bundles");
        m.insert(JitoRegion::Tokyo, "https://tokyo.mainnet.block-engine.jito.wtf/api/v1/bundles");
        m
    };

    // --- VOS LATENCES ESTIMÉES (À METTRE À JOUR SUR LE BARE METAL) ---
    static ref ESTIMATED_LATENCIES: HashMap<JitoRegion, u32> = {
        let mut m = HashMap::new();
        // Europe
        m.insert(JitoRegion::Frankfurt, 15);
        m.insert(JitoRegion::Amsterdam, 25);
        m.insert(JitoRegion::London, 30);
        m.insert(JitoRegion::Dublin, 40);
        // US
        m.insert(JitoRegion::NewYork, 90);
        m.insert(JitoRegion::SaltLakeCity, 130);
        // Asie
        m.insert(JitoRegion::Tokyo, 200);
        m.insert(JitoRegion::Singapore, 220);
        m
    };

    // --- NIVEAU 1 : MAPPAGE VILLE -> RÉGION (ENRICHI GRÂCE À VOS DONNÉES) ---
    static ref CITY_TO_REGION: HashMap<&'static str, JitoRegion> = {
        let mut m = HashMap::new();
        // Europe
        m.insert("Frankfurt", JitoRegion::Frankfurt);
        m.insert("Rüsselsheim", JitoRegion::Frankfurt);
        m.insert("Fechenheim", JitoRegion::Frankfurt);
        m.insert("Hattersheim", JitoRegion::Frankfurt);
        m.insert("Offenbach", JitoRegion::Frankfurt);
        m.insert("Amsterdam", JitoRegion::Amsterdam);
        m.insert("Haarlem", JitoRegion::Amsterdam);
        m.insert("Rotterdam", JitoRegion::Amsterdam);
        m.insert("London", JitoRegion::London);
        m.insert("Tower Hamlets", JitoRegion::London);
        m.insert("Coventry", JitoRegion::London);
        m.insert("Dublin", JitoRegion::Dublin);
        m.insert("Helsinki", JitoRegion::Frankfurt);
        m.insert("Warsaw", JitoRegion::Frankfurt);
        m.insert("Roubaix", JitoRegion::Frankfurt);
        m.insert("Strasbourg", JitoRegion::Frankfurt);
        m.insert("Vilnius", JitoRegion::Frankfurt);
        m.insert("Stockholm", JitoRegion::Frankfurt);
        m.insert("Tirana", JitoRegion::Frankfurt);
        m.insert("Oslo", JitoRegion::Frankfurt);
        m.insert("Prague", JitoRegion::Frankfurt);
        m.insert("Moscow", JitoRegion::Frankfurt); // Le BE le plus proche est en Europe
        m.insert("Kyiv", JitoRegion::Frankfurt);
        m.insert("Riga", JitoRegion::Frankfurt);
        m.insert("Vienna", JitoRegion::Frankfurt);

        // Amérique du Nord
        m.insert("New York", JitoRegion::NewYork);
        m.insert("Newark", JitoRegion::NewYork);
        m.insert("Piscataway", JitoRegion::NewYork);
        m.insert("Staten Island", JitoRegion::NewYork);
        m.insert("Chicago", JitoRegion::NewYork);
        m.insert("Elk Grove Village", JitoRegion::NewYork);
        m.insert("Ashburn", JitoRegion::NewYork);
        m.insert("Toronto", JitoRegion::NewYork);
        m.insert("Beauharnois", JitoRegion::NewYork);
        m.insert("Salt Lake City", JitoRegion::SaltLakeCity);
        m.insert("Ogden", JitoRegion::SaltLakeCity);
        m.insert("Draper", JitoRegion::SaltLakeCity);
        m.insert("Bluffdale", JitoRegion::SaltLakeCity);

        // Asie
        m.insert("Tokyo", JitoRegion::Tokyo);
        m.insert("Singapore", JitoRegion::Singapore);
        m.insert("Hong Kong", JitoRegion::Singapore); // Singapore est souvent le meilleur hub
        m.insert("Gurugram", JitoRegion::Singapore);

        // Autres
        m.insert("Mexico City", JitoRegion::NewYork);
        m.insert("São Paulo", JitoRegion::NewYork);
        m.insert("Santiago", JitoRegion::NewYork);
        m.insert("Isando", JitoRegion::Frankfurt); // Afrique du Sud -> Europe
        m.insert("Port Elizabeth", JitoRegion::Frankfurt);
        m.insert("Dubai", JitoRegion::Frankfurt);
        m
    };

    // --- NIVEAU 2 : MAPPAGE PAYS -> RÉGION (FALLBACK) ---
    static ref COUNTRY_TO_REGION: HashMap<&'static str, JitoRegion> = {
        let mut m = HashMap::new();
        // Europe
        m.insert("DE", JitoRegion::Frankfurt); m.insert("NL", JitoRegion::Amsterdam);
        m.insert("GB", JitoRegion::London); m.insert("IE", JitoRegion::Dublin);
        m.insert("FR", JitoRegion::Frankfurt); m.insert("FI", JitoRegion::Frankfurt);
        m.insert("PL", JitoRegion::Frankfurt); m.insert("LT", JitoRegion::Frankfurt);
        m.insert("SE", JitoRegion::Frankfurt); m.insert("AL", JitoRegion::Frankfurt);
        m.insert("NO", JitoRegion::Frankfurt); m.insert("CZ", JitoRegion::Frankfurt);
        m.insert("RU", JitoRegion::Frankfurt); m.insert("UA", JitoRegion::Frankfurt);
        m.insert("LV", JitoRegion::Frankfurt); m.insert("AT", JitoRegion::Frankfurt);
        m.insert("LU", JitoRegion::Frankfurt); m.insert("CH", JitoRegion::Frankfurt);
        m.insert("ES", JitoRegion::Frankfurt); m.insert("RO", JitoRegion::Frankfurt);
        m.insert("SK", JitoRegion::Frankfurt);

        // Amérique
        m.insert("US", JitoRegion::NewYork); // Par défaut, on vise la côte Est
        m.insert("CA", JitoRegion::NewYork);
        m.insert("BR", JitoRegion::NewYork);
        m.insert("MX", JitoRegion::NewYork);
        m.insert("CL", JitoRegion::NewYork);

        // Asie
        m.insert("JP", JitoRegion::Tokyo);
        m.insert("SG", JitoRegion::Singapore);
        m.insert("HK", JitoRegion::Singapore);
        m.insert("IN", JitoRegion::Singapore);

        // Autres
        m.insert("ZA", JitoRegion::Frankfurt); // Afrique du Sud
        m.insert("AE", JitoRegion::Frankfurt); // Emirats Arabes Unis
        m
    };
}

pub fn get_routing_info(validator_intel: &ValidatorIntel) -> Option<RoutingInfo> {
    let dc_key = validator_intel.data_center_key.as_deref().unwrap_or_default();

    // Gérer le cas "Unknown"
    if dc_key == "0--Unknown" || dc_key.is_empty() {
        return None;
    }

    let parts: Vec<&str> = dc_key.split('-').collect();

    // Niveau 1: Recherche par ville (plus robuste)
    if let Some(city_part) = parts.get(2) {
        for (city_keyword, region) in CITY_TO_REGION.iter() {
            if city_part.contains(city_keyword) {
                let latency = ESTIMATED_LATENCIES.get(region).cloned().unwrap_or(u32::MAX);
                return Some(RoutingInfo { region: *region, estimated_latency_ms: latency });
            }
        }
    }

    // Niveau 2: Recherche par pays
    if let Some(country_code) = parts.get(1) {
        if let Some(region) = COUNTRY_TO_REGION.get(country_code) {
            let latency = ESTIMATED_LATENCIES.get(region).cloned().unwrap_or(u32::MAX);
            return Some(RoutingInfo { region: *region, estimated_latency_ms: latency });
        }
    }

    // Niveau 3: Aucun mappage trouvé
    None
}