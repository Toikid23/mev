use bytemuck::{Pod, Zeroable, from_bytes};
use solana_sdk::pubkey::Pubkey;
use anyhow::{Result, bail};

#[repr(C, packed)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct TickArrayBitmapExtensionData {
    pub pool_id: Pubkey,
    pub positive_tick_array_bitmap: [[u64; 8]; 14],
    pub negative_tick_array_bitmap: [[u64; 8]; 14],
}

#[derive(Debug, Clone)]
pub struct DecodedTickArrayBitmapExtension {
    pub bitmap_words: Vec<u64>,
}

pub fn decode_tick_array_bitmap_extension(data: &[u8]) -> Result<DecodedTickArrayBitmapExtension> {
    const DISCRIMINATOR: [u8; 8] = [60, 150, 36, 219, 97, 128, 139, 153];
    if data.get(..8) != Some(&DISCRIMINATOR) {
        bail!("Discriminator de TickArrayBitmapExtension invalide.");
    }

    let data_slice = &data[8..];
    if data_slice.len() < size_of::<TickArrayBitmapExtensionData>() {
        bail!("Données de TickArrayBitmapExtension trop courtes.");
    }

    let bitmap_struct: &TickArrayBitmapExtensionData = from_bytes(&data_slice[..size_of::<TickArrayBitmapExtensionData>()]);

    // On copie les données pour éviter les problèmes d'alignement
    let negative_bitmap = bitmap_struct.negative_tick_array_bitmap;
    let positive_bitmap = bitmap_struct.positive_tick_array_bitmap;

    let mut all_bitmaps = Vec::new();
    for word_array in negative_bitmap.iter() {
        all_bitmaps.extend_from_slice(word_array);
    }
    for word_array in positive_bitmap.iter() {
        all_bitmaps.extend_from_slice(word_array);
    }

    Ok(DecodedTickArrayBitmapExtension {
        bitmap_words: all_bitmaps,
    })
}

pub fn get_bitmap_extension_address(pool_id: &Pubkey, program_id: &Pubkey) -> Pubkey {
    let (pda, _) = Pubkey::find_program_address(
        &[
            b"pool_tick_array_bitmap_extension",
            pool_id.as_ref(),
        ],
        program_id,
    );
    pda
}