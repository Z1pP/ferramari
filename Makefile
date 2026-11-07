PREFIX  ?= /usr/local
DESTDIR ?=
CARGO   ?= cargo
TARGET_DIR ?= target

APP_ID     = io.github.ferramari.Ferramari
BINDIR     = $(DESTDIR)$(PREFIX)/bin
APP_EXEC   = $(PREFIX)/bin/ferramari
DATADIR    = $(DESTDIR)$(PREFIX)/share
APPDIR     = $(DATADIR)/applications
ICONDIR    = $(DATADIR)/icons/hicolor/scalable/apps
METAINFODIR= $(DATADIR)/metainfo
LOCALEDIR  = $(DATADIR)/locale

.PHONY: all build release test lint fmt clean install uninstall run translations bench

all: build

# Ferramari is a two-process application: the wallpaper server and the Iced
# control surface must always be built and shipped together.
build: release

release:
	$(CARGO) build --workspace --release --locked

test:
	$(CARGO) test --workspace --locked

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings

run:
	$(CARGO) run -p ferramari -- -d

# Usage: make bench VIDEO=/path/to/video.mp4 [PROFILE=balanced] [SECONDS=10]
#        make bench MODE=web WEB=/path/to/page.html [SECONDS=10]
VIDEO ?=
WEB ?=
MODE ?= video
LABEL ?=
PROFILE ?= balanced
SECONDS ?= 10
bench: release
	@test -n "$(VIDEO)$(WEB)" || (echo "usage: make bench VIDEO=/path/to/video.mp4"; exit 2)
	python3 scripts/benchmark.py --binary "$(TARGET_DIR)/release/ferramari" \
		--mode "$(MODE)" --profile "$(PROFILE)" --seconds "$(SECONDS)" \
		$(if $(VIDEO),--video "$(VIDEO)") $(if $(WEB),--web "$(WEB)") \
		$(if $(LABEL),--label "$(LABEL)")

translations:
	@for po in po/*.po; do \
		lang=$$(basename "$$po" .po); \
		mkdir -p "$(LOCALEDIR)/$$lang/LC_MESSAGES"; \
		msgfmt -c -o "$(LOCALEDIR)/$$lang/LC_MESSAGES/ferramari.mo" "$$po"; \
	done

install: build
	install -Dm755 "$(TARGET_DIR)/release/ferramari" "$(BINDIR)/ferramari"
	install -Dm755 "$(TARGET_DIR)/release/ferramari-ui" "$(BINDIR)/ferramari-ui"
	install -Dm644 "data/$(APP_ID).desktop" "$(APPDIR)/$(APP_ID).desktop"
	sed -i 's|^Exec=.*|Exec=$(APP_EXEC)|' "$(APPDIR)/$(APP_ID).desktop"
	install -Dm644 "data/$(APP_ID).metainfo.xml" "$(METAINFODIR)/$(APP_ID).metainfo.xml"
	install -Dm644 "data/icons/hicolor/scalable/apps/$(APP_ID).svg" "$(ICONDIR)/$(APP_ID).svg"
	@if command -v msgfmt >/dev/null 2>&1; then $(MAKE) translations; \
	else echo "msgfmt not found: install gettext to build the translations"; fi
	@if command -v gtk-update-icon-cache >/dev/null 2>&1; then \
		gtk-update-icon-cache -qtf "$(DATADIR)/icons/hicolor" 2>/dev/null || true; fi

uninstall:
	rm -f "$(BINDIR)/ferramari" "$(BINDIR)/ferramari-ui"
	rm -f "$(APPDIR)/$(APP_ID).desktop"
	rm -f "$(METAINFODIR)/$(APP_ID).metainfo.xml"
	rm -f "$(ICONDIR)/$(APP_ID).svg"
	@for po in po/*.po; do \
		lang=$$(basename "$$po" .po); \
		rm -f "$(LOCALEDIR)/$$lang/LC_MESSAGES/ferramari.mo"; \
	done

clean:
	$(CARGO) clean
