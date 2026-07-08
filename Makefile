SHELL := bash
.ONESHELL:
.SHELLFLAGS := -eu -o pipefail -c

# Publish order matters: dependencies first.
CRATES := nanostores-macros nanostores nanostores-wasm

.PHONY: help test test-wasm test-browser test-js doc build-example release publish-crates publish-npm

help:
	@echo "make test           - cargo test + clippy (native)"
	@echo "make test-wasm      - wasm32 build + clippy"
	@echo "make test-browser   - wasm-pack tests (headless chrome)"
	@echo "make test-js        - rebuild wasm package + npm typecheck + vitest + production build"
	@echo "make doc            - cargo doc"
	@echo "make build-example  - rebuild browser-app wasm package"
	@echo "make release        - interactive tag-driven release"
	@echo "make publish-crates - publish all crates to crates.io in dependency order (used by CI)"
	@echo "make publish-npm    - publish packages/nanostores-wasm to npm (used by CI)"

test:
	cargo test --workspace
	cargo clippy --workspace --all-targets -- -D warnings

test-wasm:
	cargo build --target wasm32-unknown-unknown -p nanostores -p nanostores-wasm -p browser-app-core
	cargo clippy --target wasm32-unknown-unknown -p nanostores -p nanostores-wasm -p browser-app-core -- -D warnings

test-browser:
	./crates/nanostores-wasm/run-browser-tests.sh

test-js:
	$(MAKE) build-example
	npm run typecheck
	npm test
	npm run build

doc:
	cargo doc --workspace --no-deps

build-example:
	wasm-pack build examples/browser-app/core --target web --out-dir ../ui/src/pkg
	node packages/nanostores-wasm/scripts/generate-wrapper.mjs examples/browser-app/ui/src/pkg/browser_app_core

# --- publish (CI) ------------------------------------------------------------
# cargo publish waits for crates.io index propagation since 1.66, so a plain
# sequential loop is enough. Requires CARGO_REGISTRY_TOKEN in the environment.

# Idempotent: re-running after a partial failure skips versions that are
# already published.
publish-crates:
	@for crate in $(CRATES); do
	  echo "-- publishing $$crate --"
	  out="$$(cargo publish -p "$$crate" --no-verify 2>&1)" && { echo "$$out"; continue; }
	  if grep -q "already uploaded\|already exists" <<< "$$out"; then
	    echo "ok $$crate: this version is already on crates.io - skipping"
	  else
	    echo "$$out"
	    exit 1
	  fi
	done

publish-npm:
	npm publish --workspace nanostores-wasm --access public

# --- release -----------------------------------------------------------------
# One version drives everything: the workspace is tagged vX.Y.Z; workspace
# package versions are bumped to the same X.Y.Z. Pushing the tag triggers
# .github/workflows/release.yml, which re-runs CI, creates a GitHub release,
# and publishes to crates.io.

release:
	@set -euo pipefail
	cd "$$(git rev-parse --show-toplevel)"

	if [ -n "$$(git status --porcelain)" ]; then
	  echo "error: working tree is not clean - commit or stash first:"
	  git status --short
	  exit 1
	fi

	cur="$$(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' | sed 's/^v//' | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
	cur="$${cur:-0.0.0}"
	manifest_cur="$$(sed -n 's/^version = "\(.*\)"$$/\1/p' Cargo.toml | head -1)"
	head="$$(git rev-parse --short HEAD)"
	echo "Latest release: v$$cur    manifest: $$manifest_cur    HEAD: $$head"
	echo
	echo "  1) bump version"
	echo "  2) recreate last tag (v$$cur) on HEAD   [force]"
	echo "  3) cancel"
	read -r -p "> " action

	set_version() {
	  new="$$1"
	  old="$$manifest_cur"
	  sed -i "0,/^version = \"$$old\"$$/s//version = \"$$new\"/" Cargo.toml
	  sed -i "/^nanostores/s/version = \"$$old\"/version = \"$$new\"/g" Cargo.toml
	  cargo check --workspace --quiet
	  npm version "$$new" --workspaces --include-workspace-root --no-git-tag-version
	}

	case "$$action" in
	1)
	  IFS=. read -r MA MI PA <<< "$$cur"
	  echo
	  echo "  1) major  -> v$$((MA+1)).0.0"
	  echo "  2) minor  -> v$$MA.$$((MI+1)).0"
	  echo "  3) patch  -> v$$MA.$$MI.$$((PA+1))"
	  read -r -p "> " comp
	  case "$$comp" in
	    1) MA=$$((MA+1)); MI=0; PA=0 ;;
	    2) MI=$$((MI+1)); PA=0 ;;
	    3) PA=$$((PA+1)) ;;
	    *) echo "Aborted."; exit 0 ;;
	  esac
	  new="$$MA.$$MI.$$PA"
	  echo
	  echo "Release v$$new - will:"
	  echo "  - set version $$new across Cargo workspace and npm packages"
	  echo "  - commit 'release v$$new'"
	  echo "  - create tag v$$new and push HEAD + tag (triggers CI crates.io + npm release)"
	  read -r -p "Type 'yes' to proceed: " ok
	  [ "$$ok" = "yes" ] || { echo "Aborted."; exit 0; }

	  set_version "$$new"
	  git add -A
	  git diff --cached --quiet || git commit -m "release v$$new"
	  git tag -a "v$$new" -m "v$$new"
	  git push origin HEAD
	  git push origin "v$$new"
	  echo "ok released v$$new."
	  ;;
	2)
	  if [ "$$cur" = "0.0.0" ] && ! git tag -l 'v0.0.0' | grep -q .; then
	    echo "error: no release tag to recreate."; exit 1
	  fi
	  echo
	  echo "Will DELETE and recreate tag v$$cur on $$head, then force-push."
	  read -r -p "Type 'yes' to proceed: " ok
	  [ "$$ok" = "yes" ] || { echo "Aborted."; exit 0; }
	  git tag -d "v$$cur" 2>/dev/null || true
	  git push origin ":refs/tags/v$$cur" 2>/dev/null || true
	  git tag -a "v$$cur" -m "v$$cur"
	  git push origin --force "v$$cur"
	  echo "ok recreated v$$cur on $$head."
	  ;;
	*)
	  echo "Cancelled."
	  ;;
	esac
