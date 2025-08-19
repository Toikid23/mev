// DANS: src/decoders/raydium_decoders/openbook_market.rs
// LA CORRECTION FINALE. PAS DE SUPPOSITIONS. JUSTE LES FAITS.

use anyhow::{anyhow, Result};
use bytemuck::from_bytes;
use openbook_dex::state::MarketState;
use openbook_dex::critbit::Slab;
use std::mem::size_of;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: u64,
    pub quantity: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBook {
    pub levels: Vec<PriceLevel>,
}

pub fn decode_order_book(
    market_data: &[u8],
    order_book_data: &[u8],
    is_bids: bool,
) -> Result<OrderBook> {
    // Le décodage du marché reste correct.
    let market: &MarketState = from_bytes(
        market_data.get(5..5 + size_of::<MarketState>())
            .ok_or_else(|| anyhow!("Données du marché trop courtes"))?
    );
    let coin_lot_size = market.coin_lot_size;
    let pc_lot_size = market.pc_lot_size;

    // --- LA CORRECTION DÉFINITIVE ---
    // La structure Slab commence APRÈS 5 octets de padding et 8 octets de flags.
    // Soit un offset total de 13 octets.
    const SLAB_OFFSET: usize = 13;

    let slab_data = order_book_data
        .get(SLAB_OFFSET..)
        .ok_or_else(|| anyhow!("Données du carnet d'ordres trop courtes pour contenir un Slab"))?;

    // On crée une copie mutable UNIQUEMENT des données du Slab.
    let mut slab_data_mut = slab_data.to_vec();
    let slab = Slab::new(&mut slab_data_mut);
    // --- FIN DE LA CORRECTION ---


    let mut levels = Vec::new();
    if is_bids {
        while let Some(leaf) = slab.remove_max() {
            levels.push(PriceLevel {
                price: leaf.price().get().checked_mul(pc_lot_size).unwrap_or(0),
                quantity: leaf.quantity().checked_mul(coin_lot_size).unwrap_or(0),
            });
        }
    } else {
        while let Some(leaf) = slab.remove_min() {
            levels.push(PriceLevel {
                price: leaf.price().get().checked_mul(pc_lot_size).unwrap_or(0),
                quantity: leaf.quantity().checked_mul(coin_lot_size).unwrap_or(0),
            });
        }
    }

    Ok(OrderBook { levels })
}