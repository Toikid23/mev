# DANS : Makefile (à la racine de votre projet)

# ==============================================================================
# Makefile pour la Gestion Complète du Bot MEV
# ==============================================================================
#
# Usage :
#   make build          - Compile tous les binaires en mode release.
#   make deploy         - Transfère les binaires sur le serveur.
#   make setup-server   - Prépare le serveur (copie les services, installe cron).
#   make start          - Démarre tous les services du bot sur le serveur.
#   make stop           - Arrête tous les services.
#   make restart        - Redémarre tous les services.
#   make status         - Affiche le statut des services.
#   make logs           - Affiche les logs de l'engine en temps réel.
#   make run-census     - Lance une tâche de recensement manuellement.
#   make run-health-check - Lance une vérification de santé manuellement.
# ==============================================================================

# --- Configuration ---
# MODIFIEZ CETTE LIGNE avec votre utilisateur et l'IP de votre serveur.
REMOTE_SSH := mev@123.45.67.89
REMOTE_DIR := /home/mev/bot

# Couleurs pour une sortie plus lisible
GREEN = \033[0;32m
YELLOW = \033[0;33m
NC = \033[0m

# --- Commandes Principales ---

.DEFAULT_GOAL := help

# Cible pour compiler tous les binaires en mode optimisé.
build:
	@echo "$(YELLOW)--- 1. Compilation de tous les binaires en mode release... ---$(NC)"
	@cargo build --release
	@echo "$(GREEN)--- Compilation terminée. ---$(NC)"

# Cible pour transférer les fichiers nécessaires sur le serveur.
deploy: build
	@echo "$(YELLOW)--- 2. Déploiement des nouveaux binaires et fichiers de config... ---$(NC)"
	@rsync -avz --progress \
		--exclude 'debug/' \
		--exclude 'deps/' \
		--exclude 'incremental/' \
		./target/release/ ${REMOTE_SSH}:${REMOTE_DIR}/target/release/
	@rsync -avz ./.env ${REMOTE_SSH}:${REMOTE_DIR}/
	@rsync -avz ./deployment/ ${REMOTE_SSH}:${REMOTE_DIR}/deployment/
	@echo "$(GREEN)--- Déploiement terminé. ---$(NC)"

# Cible à exécuter UNE SEULE FOIS pour préparer le serveur.
setup-server:
	@echo "$(YELLOW)--- 3. Préparation initiale du serveur (à n'exécuter qu'une fois)... ---$(NC)"
	@echo "Copie des fichiers de service systemd..."
	@ssh ${REMOTE_SSH} "sudo cp ${REMOTE_DIR}/deployment/*.service /etc/systemd/system/"
	@echo "Rechargement du daemon systemd..."
	@ssh ${REMOTE_SSH} "sudo systemctl daemon-reload"
	@echo "Activation des services pour le démarrage automatique..."
	@ssh ${REMOTE_SSH} "sudo systemctl enable mev-gateway.service mev-scanner.service arbitrage-engine.service"
	@echo "Installation des tâches cron..."
	@ssh ${REMOTE_SSH} "crontab ${REMOTE_DIR}/deployment/crontab.txt"
	@echo "$(GREEN)--- Le serveur est prêt. Vous pouvez maintenant utiliser 'make start'. ---$(NC)"

# --- Commandes de Gestion des Services ---

start:
	@echo "$(YELLOW)--- Démarrage de tous les services du bot... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo systemctl start mev-gateway.service mev-scanner.service mev-copytrade.service mev-engine.service"
	@make status

stop:
	@echo "$(YELLOW)--- Arrêt de tous les services du bot... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo systemctl stop mev-gateway.service mev-scanner.service mev-copytrade.service mev-engine.service"
	@make status

restart:
	@echo "$(YELLOW)--- Redémarrage de tous les services du bot... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo systemctl restart mev-gateway.service mev-scanner.service mev-copytrade.service mev-engine.service"
	@echo "Attente de 5 secondes pour la stabilisation des services..."
	@sleep 5
	@make status

status:
	@echo "$(YELLOW)--- Statut des services du bot : ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo systemctl status mev-*.service --no-pager"

logs: logs-engine

# Affiche les logs de l'arbitrage_engine en temps réel
logs-engine:
	@echo "$(YELLOW)--- Affichage des logs de l'ARBITRAGE ENGINE en direct (Ctrl+C pour quitter)... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo journalctl -u arbitrage-engine.service -f -n 100"

# Affiche les logs du geyser_gateway en temps réel
logs-gateway:
	@echo "$(YELLOW)--- Affichage des logs du GEYSER GATEWAY en direct (Ctrl+C pour quitter)... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo journalctl -u mev-gateway.service -f -n 100"

# Affiche les logs du market_scanner en temps réel
logs-scanner:
	@echo "$(YELLOW)--- Affichage des logs du MARKET SCANNER en direct (Ctrl+C pour quitter)... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo journalctl -u mev-scanner.service -f -n 100"

logs-copytrade:
	@echo "$(YELLOW)--- Affichage des logs du COPY-TRADE TRACKER en direct (Ctrl+C pour quitter)... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo journalctl -u mev-copytrade.service -f -n 100"

# Affiche les logs du worker de maintenance (utile pour le débogage de cron)
logs-worker:
	@echo "$(YELLOW)--- Affichage des 100 derniers logs du MAINTENANCE WORKER... ---$(NC)"
	@ssh ${REMOTE_SSH} "cat ${REMOTE_DIR}/logs/maintenance_worker.log | tail -n 100"

# Cible spéciale pour voir TOUS les logs de TOUS les services en même temps, entrelacés
logs-all:
	@echo "$(YELLOW)--- Affichage de TOUS les logs en direct (Ctrl+C pour quitter)... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo journalctl -u mev-*.service -f -n 100"

# --- Commandes pour les Tâches Manuelles ---

run-census:
	@echo "$(YELLOW)--- Lancement d'une tâche de recensement manuelle sur le serveur... ---$(NC)"
	@ssh ${REMOTE_SSH} "${REMOTE_DIR}/target/release/maintenance_worker census"

run-health-check:
	@echo "$(YELLOW)--- Lancement d'une vérification de santé manuelle sur le serveur... ---$(NC)"
	@ssh ${REMOTE_SSH} "${REMOTE_DIR}/target/release/maintenance_worker health-check"

run-update-config:
	@echo "$(YELLOW)--- Lancement d'une mise à jour de la config manuelle sur le serveur... ---$(NC)"
	@ssh ${REMOTE_SSH} "${REMOTE_DIR}/target/release/maintenance_worker update-runtime-config"

# --- Contrôle des Stratégies de Découverte ---

strategy-volume-enable:
	@echo "$(YELLOW)--- Activation de la stratégie de scan par volume... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo systemctl enable --now mev-scanner.service"
	@make status

strategy-volume-disable:
	@echo "$(YELLOW)--- Désactivation de la stratégie de scan par volume... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo systemctl disable --now mev-scanner.service"
	@make status

strategy-copytrade-enable:
	@echo "$(YELLOW)--- Activation de la stratégie de copy-trading... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo systemctl enable --now mev-copytrade.service"
	@make status

strategy-copytrade-disable:
	@echo "$(YELLOW)--- Désactivation de la stratégie de copy-trading... ---$(NC)"
	@ssh ${REMOTE_SSH} "sudo systemctl disable --now mev-copytrade.service"
	@make status


# --- Cible d'Aide ---
help:
	@echo "Makefile pour la gestion du Bot MEV. Commandes disponibles:"
	@echo ""
	@echo "  --- Build & Déploiement ---"
	@echo "  build              - Compile le projet en mode release."
	@echo "  deploy             - Déploie les nouveaux binaires et configs sur le serveur."
	@echo "  setup-server       - Configure systemd et cron sur un nouveau serveur (à lancer une fois)."
	@echo ""
	@echo "  --- Contrôle des Services ---"
	@echo "  start              - Démarre tous les services du bot."
	@echo "  stop               - Arrête tous les services du bot."
	@echo "  restart            - Redémarre tous les services."
	@echo "  status             - Affiche le statut des services."
	@echo ""
	@echo "  --- Consultation des Logs ---"
	@echo "  logs (ou logs-engine) - Affiche les logs de l'arbitrage_engine."
	@echo "  logs-gateway       - Affiche les logs du geyser_gateway."
	@echo "  logs-scanner       - Affiche les logs du market_scanner."
	@echo "  logs-worker        - Affiche les derniers logs du worker de maintenance."
	@echo "  logs-all           - Affiche tous les logs de tous les services en même temps."
	@echo ""
	@echo "  --- Tâches Manuelles ---"
	@echo "  run-census         - Lance manuellement un recensement."
	@echo "  run-health-check   - Lance manuellement une vérification de santé."
	@echo "  run-update-config  - Lance manuellement une mise à jour de la config."
	@echo ""
    @echo "  --- Contrôle des Stratégies de Découverte ---"
    @echo "  strategy-volume-enable      - Active le scan par volume."
    @echo "  strategy-volume-disable     - Désactive le scan par volume."
    @echo "  strategy-copytrade-enable   - Active le copy-trading."
    @echo "  strategy-copytrade-disable  - Désactive le copy-trading."
    @echo "  (La stratégie manuelle est toujours active via le fichier manual_hotlist.json)"


# Déclare que ces cibles ne sont pas des fichiers, pour que `make` les exécute toujours.
.PHONY: build deploy setup-server start stop restart status logs logs-engine logs-gateway logs-scanner logs-worker logs-all run-census run-health-check run-update-config strategy-volume-enable strategy-volume-disable strategy-copytrade-enable strategy-copytrade-disable help