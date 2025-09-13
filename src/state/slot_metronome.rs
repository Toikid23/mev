// DANS : src/state/slot_metronome.rs (Version Corrigée)

use super::slot_tracker::{SlotState, SlotTracker}; // On importe SlotState
use std::{
    collections::VecDeque,
    sync::{Arc, RwLock},
};
use tokio::task::JoinHandle;

const SLOT_HISTORY_SIZE: usize = 100;

#[derive(Debug, Clone)]
struct SlotTiming {
    duration_ms: u128,
}

/// Analyse le flux de slots pour fournir des métriques de temps exploitables.
#[derive(Clone)]
pub struct SlotMetronome {
    slot_tracker: Arc<SlotTracker>,
    history: Arc<RwLock<VecDeque<SlotTiming>>>,
    // Stocke l'état du slot précédent pour calculer la durée.
    last_slot_state: Arc<RwLock<SlotState>>,
}

impl SlotMetronome {
    pub fn new(slot_tracker: Arc<SlotTracker>) -> Self {
        let initial_state = slot_tracker.current();
        Self {
            slot_tracker,
            history: Arc::new(RwLock::new(VecDeque::with_capacity(SLOT_HISTORY_SIZE))),
            last_slot_state: Arc::new(RwLock::new((*initial_state).clone())),
        }
    }

    /// Lance la tâche de fond qui analyse les changements de slot et met à jour l'historique.
    pub fn start(&self) -> JoinHandle<()> {
        let self_clone = self.clone();
        tokio::spawn(async move {
            loop {
                // Attente active mais légère pour détecter le changement de slot.
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

                let current_slot_state_arc = self_clone.slot_tracker.current();
                let mut last_slot_writer = self_clone.last_slot_state.write().unwrap();

                if current_slot_state_arc.clock.slot > last_slot_writer.clock.slot {
                    // Calcul de la durée du slot qui vient de se terminer
                    let duration = current_slot_state_arc.received_at.duration_since(last_slot_writer.received_at);

                    let new_timing = SlotTiming {
                        duration_ms: duration.as_millis(),
                    };

                    { // Bloc pour le write lock sur l'historique
                        let mut history_writer = self_clone.history.write().unwrap();
                        if history_writer.len() == SLOT_HISTORY_SIZE {
                            history_writer.pop_front();
                        }
                        history_writer.push_back(new_timing);
                    } // Le write lock est libéré ici

                    // On met à jour le dernier état connu avec l'état actuel
                    *last_slot_writer = (*current_slot_state_arc).clone();
                }
            }
        })
    }

    /// Calcule la durée moyenne des slots sur la base de l'historique récent.
    pub fn average_slot_duration_ms(&self) -> u128 {
        let history_reader = self.history.read().unwrap();
        if history_reader.is_empty() {
            return 450; // Estimation par défaut si l'historique est vide
        }
        let sum: u128 = history_reader.iter().map(|timing| timing.duration_ms).sum();
        sum / history_reader.len() as u128
    }

    /// Retourne le temps écoulé en millisecondes depuis le début du slot actuel.
    pub fn time_elapsed_in_current_slot_ms(&self) -> u128 {
        self.slot_tracker.current().received_at.elapsed().as_millis()
    }

    /// Estime le temps restant dans le slot actuel en se basant sur la durée moyenne.
    pub fn estimated_time_remaining_in_slot_ms(&self) -> u128 {
        let avg_duration = self.average_slot_duration_ms();
        let elapsed = self.time_elapsed_in_current_slot_ms();
        avg_duration.saturating_sub(elapsed)
    }
}