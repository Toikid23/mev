use anyhow::Result;
use serde::Deserialize;

const ORCA_API_URL: &str = "https://api.orca.so/v2/solana/pools";


#[derive(Deserialize, Debug)]
struct OrcaApiV2Response {
    data: Vec<OrcaPoolInfo>,
    meta: Option<ApiMeta>, // `meta` peut être absent sur la dernière page
}

// La structure `meta` qui contient le curseur pour la pagination.
#[derive(Deserialize, Debug)]
struct ApiMeta {
    next: Option<String>, // Le curseur est un `String`, et il est optionnel.
}

// Les informations qui nous intéressent pour chaque pool (whirlpool).
// `#[serde(rename_all = "camelCase")]` est nécessaire pour les champs comme `tokenMintA`.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OrcaPoolInfo {
    pub address: String,
    pub token_mint_a: String,
    pub token_mint_b: String,
}

// Interroge l'API d'Orca pour récupérer la liste complète des whirlpools.
// Gère la pagination par curseur (`next`).
pub async fn fetch_orca_pools() -> Result<Vec<OrcaPoolInfo>> {
    println!("\nLancement de la récupération de tous les whirlpools depuis l'API Orca V2 (avec pagination)...");

    let mut all_pools = Vec::new();
    let mut next_cursor: Option<String> = None;
    let client = reqwest::Client::new();

    loop {
        // On demande une grande taille de page pour minimiser le nombre d'appels.
        let mut url = format!("{}?size=3000", ORCA_API_URL);

        // Si nous avons un curseur de la réponse précédente, nous l'ajoutons à l'URL.
        if let Some(cursor) = &next_cursor {
            url.push_str(&format!("&next={}", cursor));
        }

        println!("Récupération d'une page depuis l'URL : {}", url);

        // On fait l'appel et on parse la réponse.
        let response = client.get(&url).send().await?.json::<OrcaApiV2Response>().await?;

        let num_fetched = response.data.len();
        all_pools.extend(response.data);

        println!("-> Page récupérée avec {} pools. Total jusqu'à présent : {}", num_fetched, all_pools.len());

        // On met à jour le curseur pour la prochaine itération.
        // Si le champ `meta` ou le champ `next` est absent, `next_cursor` deviendra `None`.
        if let Some(meta) = response.meta {
            next_cursor = meta.next;
        } else {
            next_cursor = None;
        }

        // Si `next_cursor` est `None`, ou si la page est vide, on a fini.
        if next_cursor.is_none() || num_fetched == 0 {
            break;
        }
    }

    println!("\n✅ Récupération terminée. Nombre total de pools trouvés sur Orca : {}", all_pools.len());
    Ok(all_pools)
}