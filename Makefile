PREFIX  ?= /usr/local
DESTDIR ?=
CARGO   ?= cargo
TARGET_DIR ?= target
PYTHON  ?= python3

# The gettext domain, which is also the basename of the installed .mo file.
APP_DOMAIN = ferramari

APP_ID     = io.github.ferramari.Ferramari
BINDIR     = $(DESTDIR)$(PREFIX)/bin
APP_EXEC   = $(PREFIX)/bin/ferramari
DATADIR    = $(DESTDIR)$(PREFIX)/share
APPDIR     = $(DATADIR)/applications
ICONDIR    = $(DATADIR)/icons/hicolor/scalable/apps
METAINFODIR= $(DATADIR)/metainfo
LOCALEDIR  = $(DATADIR)/locale

.PHONY: all build release test lint fmt clean install uninstall run translations bench \
       pot po-update po-fill po-status po-check

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
	$(CARGO) fmt --all
	git diff --check
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings

run:
	$(CARGO) run -p ferramari -- -d

# Usage: make bench VIDEO=/path/to/video.mp4 [PROFILE=balanced] [SECONDS=10]
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

# The catalogue template is generated, not maintained by hand: GNU gettext
# has no Rust parser and its Python mode misreads Rust-only syntax, so
# `scripts/extract-messages.py` walks the sources itself. Run this after
# adding or renaming any `tr("…")`, then `make po-update`.
pot:
	$(PYTHON) scripts/extract-messages.py -o po/$(APP_DOMAIN).pot

# Folds the new template into every language. msgmerge never invents a
# finished translation: strings it is unsure about arrive flagged `#, fuzzy`,
# which gettext ignores at run time, so the UI falls back to English rather
# than showing a wrong sentence. `make po-fill` turns those into real ones.
po-update: pot
	@for po in po/*.po; do \
		msgmerge --quiet --update --backup=none "$$po" po/$(APP_DOMAIN).pot; \
	done

# Applies the reviewed wording held in `scripts/translations-<lang>.py` to
# the entries that are still fuzzy or empty. A language with no table is left
# untouched and reported, so adding a locale is one file, not an edit here.
po-fill:
	@for po in po/*.po; do \
		lang=$$(basename "$$po" .po); \
		table=scripts/translations-$$lang.py; \
		if [ -f "$$table" ]; then \
			$(PYTHON) scripts/fill-po.py "$$po" --table "$$table"; \
		else \
			echo "$$po: no $$table, leaving it to a translator"; \
		fi; \
	done

# What is still open in each language, as counted by gettext itself.
po-status:
	@for po in po/*.po; do printf '%-10s ' "$$(basename $$po .po)"; \
		msgfmt --statistics -c -o /dev/null "$$po" 2>&1 | tail -1; done

# The template is checked, not trusted: a string added to a page and never
# added to the catalogue is how a translation quietly goes stale.
po-check:
	$(PYTHON) scripts/extract-messages.py -o po/$(APP_DOMAIN).pot --check

install: build
	install -Dm755 "$(TARGET_DIR)/release/ferramari" "$(BINDIR)/ferramari"
	install -Dm755 "$(TARGET_DIR)/release/ferramari-ui" "$(BINDIR)/ferramari-ui"
	install -Dm644 "data/$(APP_ID).desktop" "$(APPDIR)/$(APP_ID).desktop"
	sed -i 's|^Exec=.*|Exec=$(APP_EXEC)|' "$(APPDIR)/$(APP_ID).desktop"
	install -Dm644 "data/$(APP_ID).metainfo.xml" "$(METAINFODIR)/$(APP_ID).metainfo.xml"
	install -Dm644 "data/icons/hicolor/scalable/apps/$(APP_ID).svg" "$(ICONDIR)/$(APP_ID).svg"
	@if command -v msgfmt >/dev/null 2>&1; then $(MAKE) translations; \
	else echo "msgfmt not found: install gettext to build the translations"; fi

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
