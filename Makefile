# Simple Makefile for Hierophant
.PHONY: hierophant hierophant-d stop logs restart check-env

check-env:
	@test -f .env || (echo "Error: .env file not found!" && exit 1)

# Start in foreground
hierophant: check-env
	docker compose -f docker-compose.yml up

# Start in background
hierophant-d: check-env
	docker compose -f docker-compose.yml up -d

# Stop 
stop:
	docker compose -f docker-compose.yml down

# View  logs
logs:
	docker compose -f docker-compose.yml logs -f

# Restart hierophant
restart:
	docker compose -f docker-compose.yml restart
