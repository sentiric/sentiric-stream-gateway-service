.PHONY: help setup up down logs test build clean

GREEN=\033[0;32m
NC=\033[0m

help: ## Komutları listeler
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

setup: ## Geliştirme ortamını hazırlar
	@echo "${GREEN}Checking certificates...${NC}"
	@if [ ! -d "../sentiric-certificates" ]; then \
		echo "❌ Error: ../sentiric-certificates dizini bulunamadı!"; \
		exit 1; \
	fi
	@echo "✅ Certificates found."

build: ## Release modunda derler
	cargo build --release

up: setup ## Servisi Docker ile başlatır
	docker compose up --build -d
	@echo "${GREEN}🚀 Service (Stream Gateway) is running at http://localhost:18030${NC}"

down: ## Servisi durdurur
	docker compose down

logs: ## Logları canlı izler
	docker compose logs -f stream-gateway-service

test: ## Birim testleri çalıştırır
	cargo test

clean: ## Temizlik yapar
	cargo clean
	docker system prune -f