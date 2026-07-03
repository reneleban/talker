# talker — Entwickler-Kommandos. `make help` zeigt alles.

APP_DEST ?= $(HOME)/Applications

.PHONY: help verify build test clippy bundle install cert stress run

help: ## Diese Übersicht
	@grep -E '^[a-z-]+:.*##' $(MAKEFILE_LIST) | awk -F ':.*## ' '{printf "  make %-10s %s\n", $$1, $$2}'

verify: ## verify:fast — Build + Tests + Clippy (vor jedem Commit)
	cargo build
	cargo test
	cargo clippy --all-targets -- -D warnings

build: ## Release-Build
	cargo build --release

test: ## Alle Tests (Unit + Integration; brauchen die Modelle)
	cargo test

clippy: ## Lint, Warnungen sind Fehler
	cargo clippy --all-targets -- -D warnings

bundle: ## talker.app bauen (signiert mit talker-dev, sonst ad-hoc)
	bash scripts/bundle.sh

cert: ## Einmalig pro Mac: talker-dev-Zertifikat anlegen (Permissions überleben Rebuilds)
	bash scripts/make-cert.sh

install: bundle ## Bundle bauen + installieren (ERSETZT das alte Bundle — nie überkopieren!)
	@if [ -d "$(APP_DEST)/talker.app" ]; then \
		mv "$(APP_DEST)/talker.app" "$(HOME)/.Trash/talker-$$(date +%s).app"; \
	fi
	cp -R target/bundle/talker.app "$(APP_DEST)/"
	@echo "✓ installiert: $(APP_DEST)/talker.app (alte Version im Papierkorb)"

stress: ## Cleanup-Stress-Testreihe (EVAL-0001) gegen das echte Modell
	cargo run --release --example cleanup_stress

stt-eval: ## STT-Qualitäts-Testreihe (EVAL-0002); Fixtures: scripts/gen_stt_fixtures.sh
	cargo run --release --example stt_eval

pipeline-ab: ## Pipeline-Vergleich A/B/C (EVAL-0003): Parakeet vs +Gemma vs Gemma-Audio
	cargo run --release --example pipeline_ab

run: ## App aus dem Quellbaum starten
	cargo run --release
