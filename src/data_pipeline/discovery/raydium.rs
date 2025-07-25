use anyhow::Result;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct RaydiumApiV3Response<T> {
    success: bool,
    data: Option<T>,
    msg: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ApiPoolsData {
    pub count: i64,
    pub data: Vec<PoolInfo>,
}


#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PoolInfo {
    pub id: String,
    pub program_id: String,
    #[serde(rename = "type")]
    pub pool_type: String,
    pub mint_a: MintInfo,
    pub mint_b: MintInfo,
    pub tvl: f64,
    pub day: VolumeInfo,
    pub config: Option<PoolConfig>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MintInfo {
    pub address: String,
    pub decimals: i32,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub volume: f64,
    pub apr: f64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PoolConfig {
    pub trade_fee_rate: i64,
}

const PAGE_SIZE: i32 = 1000;

// Interroge l'API V3 de Raydium et gère la pagination pour récupérer TOUS les pools.
pub async fn fetch_raydium_pools() -> Result<Vec<PoolInfo>> {
    let client = reqwest::Client::new();
    let mut all_pools: Vec<PoolInfo> = Vec::new();
    let mut current_page = 1;

    println!("\nLancement de la récupération de tous les pools depuis l'API Raydium (avec pagination)...");

    loop {
        let url = format!(
            "https://api-v3.raydium.io/pools/info/list?poolType=all&poolSortField=default&sortType=desc&pageSize={}&page={}",
            PAGE_SIZE, current_page
        );

        println!("Récupération de la page {}...", current_page);

        // Une gestion d'erreur plus robuste pour le parsing JSON
        let raw_text = client.get(&url).send().await?.text().await?;
        let response_body: RaydiumApiV3Response<ApiPoolsData> = match serde_json::from_str(&raw_text) {
            Ok(body) => body,
            Err(e) => {
                // Si le parsing échoue, on affiche le début de la réponse pour aider au debug.
                println!("--- ERREUR DE DÉCODAGE JSON ---");
                println!("Réponse brute (2000 premiers caractères): {}", &raw_text[..std::cmp::min(2000, raw_text.len())]);
                println!("-----------------------------");
                return Err(e.into()); // On propage l'erreur
            }
        };

        if !response_body.success {
            let error_msg = response_body.msg.unwrap_or_else(|| "Erreur API inconnue".to_string());
            return Err(anyhow::anyhow!("L'API Raydium a retourné une erreur: {}", error_msg));
        }

        if let Some(api_data) = response_body.data {
            if current_page == 1 {
                println!("Nombre total de pools disponibles selon l'API : {}", api_data.count);
            }
            let num_pools_on_page = api_data.data.len();
            println!("-> Page {} récupérée avec {} pools. Total jusqu'à présent : {}", current_page, num_pools_on_page, all_pools.len() + num_pools_on_page);
            all_pools.extend(api_data.data);

            if num_pools_on_page < PAGE_SIZE as usize {
                break;
            }
        } else {
            break;
        }

        current_page += 1;
    }

    println!("\n✅ Récupération terminée. Nombre total de pools Raydium trouvés : {}", all_pools.len());
    Ok(all_pools)
}