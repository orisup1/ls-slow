CARGO ?= cargo

.PHONY: install
install:
	$(CARGO) install --path . --locked
