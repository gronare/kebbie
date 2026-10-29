#!/bin/bash
# Builds a throwaway kebbie world under $KDEMO_DIR: its own HOME, herdr server, repos and a stand-in claude.
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
D=${KDEMO_DIR:-$(cd /tmp && pwd -P)/kebbie-demo}
H=$D/home

case "$D" in /tmp/*|/private/tmp/*) ;; *) echo "KDEMO_DIR must be under /tmp" >&2; exit 1 ;; esac

demo_socket() { "$D/env.sh" herdr status --json 2>/dev/null | jq -r .server.socket; }

stop_demo() {
  [ -x "$D/env.sh" ] || return 0
  if [ "$(demo_socket)" = "$H/.config/herdr/herdr.sock" ]; then "$D/env.sh" herdr server stop >/dev/null 2>&1 || true; fi
  local pid; pid=$(cat "$D/server.pid" 2>/dev/null || true)
  if [ -n "$pid" ] && ps -p "$pid" -o command= | grep -q '^herdr server'; then kill "$pid"; fi
  pid=$(cat "$D/releases.pid" 2>/dev/null || true)
  if [ -n "$pid" ] && ps -p "$pid" -o command= | grep -q 'http.server'; then kill "$pid"; fi
  pid=$(cat "$D/hub.pid" 2>/dev/null || true)
  if [ -n "$pid" ] && ps -p "$pid" -o command= | grep -q 'kebbie-hub'; then kill "$pid"; fi
  pkill -f "$D/bin/kebbie -C .* review --run" 2>/dev/null && sleep 0.5
  pkill -f "$D/bin/claude.py" 2>/dev/null || true
  pkill -f "$D/home/.local/share/.* keeper" 2>/dev/null && sleep 0.5
  return 0
}

stop_demo
[ "${1:-}" = --stop ] && exit 0
scene=${1:-demo}
case "$scene" in demo) ;; *) [ -f "$repo_root/demo/clips/$scene.tape" ] || { echo "no scene $scene: demo or a name from demo/clips" >&2; exit 1; } ;; esac
rm -rf "$D"
mkdir -p "$D/bin" "$D/remotes" "$D/screens" "$D/modes" "$D/react" "$D/shots" "$D/verdicts" "$H/code" "$H/.config/herdr" "$H/.config/kebbie" "$H/.local/state" "$H/.local/share" "$H/.cache"

install=$H/.local/share/kebbie/current
target=$(dirname "$D")/kebbie-demo-target
cargo_env=
releases_env=
demo_path=$D/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
if [ "$scene" = update-and-restarts ]; then
  # kebbie installed as herdr installs it, a checkout at a release tag, with a newer release on its own origin to update to.
  install=$H/.local/share/herdr/plugins/kebbie
  version=$(git -C "$repo_root" show HEAD:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
  next=${version%.*}.$((${version##*.} + 1))
  git clone -q --bare --single-branch --no-tags "$repo_root" "$D/remotes/kebbie.git"
  git clone -q "$D/remotes/kebbie.git" "$D/kebbie-next"
  demo_git() { git -C "$D/kebbie-next" -c user.name="Robin Demo" -c user.email=robin@example.com "$@"; }
  # The release kebbie starts at lacks the notes the newer one adds, so the update has something new to show.
  python3 "$repo_root/demo/release-notes.py" "$D/kebbie-next/CHANGES.md" --drop
  demo_git commit -qam "kebbie $version"
  demo_git tag "v$version"
  demo_git push -q origin HEAD "v$version"
  git -C "$repo_root" show HEAD:CHANGES.md > "$D/kebbie-next/CHANGES.md"
  perl -0pi -e "s/^version = \"\Q$version\E\"/version = \"$next\"/m" "$D/kebbie-next/Cargo.toml" "$D/kebbie-next/herdr-plugin.toml"
  perl -0pi -e "s/(name = \"kebbie\"\nversion = \")\Q$version\E/\${1}$next/" "$D/kebbie-next/Cargo.lock"
  perl -pi -e "s/\"version\": \"\Q$version\E\"/\"version\": \"$next\"/" "$D/kebbie-next/.claude-plugin/plugin.json"
  python3 "$repo_root/demo/release-notes.py" "$D/kebbie-next/CHANGES.md" "$next"
  demo_git commit -qam "kebbie $next"
  demo_git tag "v$next"
  demo_git push -q origin HEAD "v$next"
  git clone -q "$D/remotes/kebbie.git" "$install"
  git -C "$install" checkout -q --detach "v$version"
  cp -R "$target" "$install/target"
  cargo build --release --quiet --manifest-path "$install/Cargo.toml"
  # The newer release's binary is published the way GitHub serves it, from a local server the update downloads it from.
  cp -R "$target" "$D/kebbie-next/target"
  cargo build --release --quiet --manifest-path "$D/kebbie-next/Cargo.toml"
  case "$(uname -s)" in Darwin) os=apple-darwin ;; *) os=unknown-linux-gnu ;; esac
  asset=kebbie-$next-$(uname -m | sed 's/^arm64$/aarch64/')-$os.tar.gz
  mkdir -p "$D/releases/v$next"
  tar -czf "$D/releases/v$next/$asset" -C "$D/kebbie-next/target/release" kebbie
  (cd "$D/releases/v$next" && { sha256sum "$asset" 2>/dev/null || shasum -a 256 "$asset"; } > SHA256SUMS)
  ( cd "$D/releases" && exec python3 -m http.server --bind 127.0.0.1 18479 > "$D/releases.log" 2>&1 & echo $! > "$D/releases.pid" )
  releases_env="KEBBIE_RELEASES_URL=http://127.0.0.1:18479"
  mkdir -p "$H/.local/share/kebbie"
  ln -s "$install" "$H/.local/share/kebbie/current"
  demo_path=$D/bin:$(dirname "$(command -v cargo)"):/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
  cargo_env="RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup} CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}"
else
  mkdir -p "$install/target/release"
  git -C "$repo_root" archive HEAD | tar -x -C "$install"
  cargo build --release --quiet --manifest-path "$install/Cargo.toml" --target-dir "$target"
  cp "$target/release/kebbie" "$install/target/release/kebbie"
fi
ln -s "$install/target/release/kebbie" "$D/bin/kebbie"
cp "$repo_root/demo/bin/claude" "$D/bin/claude.py"
cp "$repo_root/demo/bin/gh" "$D/bin/gh"
python=$(python3 -c 'import os, sys; print(os.path.realpath(sys.executable))')
app="$(dirname "$(dirname "$python")")/Resources/Python.app/Contents/MacOS/Python"
[ -x "$app" ] && python=$app
# kebbie tells the lead and a task by their process, a claude with its role's --settings, so the stand-in runs under that name; macOS's python3 stub would rename it, hence the app binary.
printf '#!/bin/bash\nexec -a claude %q %q "$@"\n' "$python" "$D/bin/claude.py" > "$D/bin/claude"
chmod +x "$D/bin/claude"

vault_env=
case "$scene" in demo|areas-page) vault_env="VAULT_PATH=$H/vault" ;; esac
cat > "$D/env.sh" <<EOF
#!/bin/sh
exec env -i HOME=$H PATH=$demo_path \\
  TERM=\${TERM:-xterm-256color} COLORTERM=truecolor LANG=en_US.UTF-8 SHELL=/bin/zsh USER=demo KDEMO_DIR=$D \\
  XDG_CONFIG_HOME=$H/.config XDG_STATE_HOME=$H/.local/state XDG_DATA_HOME=$H/.local/share XDG_CACHE_HOME=$H/.cache \\
  KEBBIE_STATE_DIR=$H/.local/state/kebbie KEBBIE_CONFIG=$H/.config/kebbie/config.toml KEBBIE_REPOS_DIR=$H/.config/kebbie/repos \\
  KEBBIE_BIN=$install/target/release/kebbie KEBBIE_CLAUDE_BIN=$D/bin/claude KEBBIE_GH_BIN=$D/bin/gh KEBBIE_REPO_DIRS=$H/code KEBBIE_IMAGES=halfblocks \\
  HERDR_SOCKET_PATH=$H/.config/herdr/herdr.sock $vault_env $cargo_env $releases_env \\
  "\$@"
EOF
chmod +x "$D/env.sh"
run() { "$D/env.sh" "$@"; }

if [ -n "$vault_env" ]; then
  mkdir -p "$H/vault" "$H/.cache/notes-vault-mcp"
  index=$(python3 -c 'import hashlib, sys; print(hashlib.sha1(sys.argv[1].encode()).hexdigest()[:12])' "$H/vault")
  claimed_by=
  [ "$scene" = demo ] && claimed_by=devbox
  python3 "$repo_root/demo/vault-index.py" "$H/.cache/notes-vault-mcp/$index.sqlite" "$H" $claimed_by
fi

chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
[ -x "$chrome" ] || chrome=$(command -v chromium || command -v google-chrome || true)
[ -n "$chrome" ] || { echo "no Chrome to render the card images with" >&2; exit 1; }
# Headless Chrome now and then exits without a screenshot, so each page gets three tries.
for page in card:saved-carts checkout:checkout-copy; do
  for _ in 1 2 3; do
    "$chrome" --headless=new --disable-gpu --hide-scrollbars --no-first-run --user-data-dir="$D/chrome" \
      --window-size=900,544 --screenshot="$D/shots/${page#*:}.png" "file://$repo_root/demo/${page%%:*}.html" >/dev/null 2>&1 && break
  done
  [ -s "$D/shots/${page#*:}.png" ] || { echo "Chrome rendered no ${page#*:}.png" >&2; exit 1; }
done
rm -rf "$D/chrome"

cat > "$H/.zshrc" <<EOF
export PATH=$demo_path
PROMPT='%F{8}%~%f %# '
EOF
cat > "$H/.gitconfig" <<'EOF'
[user]
	name = Robin Demo
	email = robin@example.com
[commit]
	gpgsign = false
[init]
	defaultBranch = main
[advice]
	detachedHead = false
EOF
# The repos' origins read as GitHub, so kebbie watches CI after a push, and git sends the pushes to the bare repos here.
printf '[url "%s/remotes/"]\n\tinsteadOf = https://github.com/robin-demo/\n' "$D" >> "$H/.gitconfig"
printf '{"projects":{"%s":{"hasTrustDialogAccepted":true}}}\n' "$H/code" > "$H/.claude.json"
cat > "$H/.config/herdr/config.toml" <<'EOF'
onboarding = false
[update]
version_check = false
manifest_check = false
[terminal]
shell_mode = "non_login"

[[keys.command]]
key = "prefix+space"
type = "shell"
command = "kebbie lead"
description = "kebbie: go to the lead, or back"

[[keys.command]]
key = "prefix+t"
type = "popup"
command = "kebbie board --overlay"
width = "85%"
height = "85%"
description = "kebbie board"
EOF
restart_auto=false
[ "$scene" = update-and-restarts ] && restart_auto=true
cat > "$H/.config/kebbie/config.toml" <<EOF
[restart]
auto = $restart_auto

[task]
steps = true
EOF
# The board's message log starts folded to one line, so the film shows every task.
mkdir -p "$H/.local/state/kebbie" && printf 'line\t0\n' > "$H/.local/state/kebbie/board-messages"
if [ "$scene" = hub-connect ]; then
  hub=${KDEMO_HUB_BIN:-$(command -v kebbie-hub || true)}
  [ -x "$hub" ] || { echo "the hub-connect clip runs a hub of its own: put kebbie-hub on PATH or set KDEMO_HUB_BIN" >&2; exit 1; }
  mkdir -p "$D/hub" && chmod 700 "$D/hub"
  ( exec "$hub" --dev --signup open --data "$D/hub" --listen 127.0.0.1:18471 --rp-id localhost --origin http://localhost:18471 > "$D/hub.log" 2>&1 & echo $! > "$D/hub.pid" )
  printf '\n[machine]\nname = "laptop"\n' >> "$H/.config/kebbie/config.toml"
fi
if [ "$scene" = demo ]; then
  cat >> "$H/.config/kebbie/config.toml" <<'EOF'

[machine]
name = "laptop"

[board.backlog]
sources = ["vault"]
EOF
  mkdir -p "$H/.config/kebbie/repos"
  printf '[worktree]\nstage_on_main = true\n' > "$H/.config/kebbie/repos/api.toml"
fi

put() { mkdir -p "$(dirname "$1/$2")"; cat > "$1/$2"; }
# The board reads the worktrees while they are seeded, so a commit retries past its index lock.
save() { local message=(-m "$2"); [ -z "${3:-}" ] || message+=(-m "$3"); for _ in 1 2 3 4 5; do run git -C "$1" add -A 2>/dev/null && run git -C "$1" commit -q "${message[@]}" 2>/dev/null && return 0; sleep 0.5; done; run git -C "$1" commit -q "${message[@]}"; }

new_repo() {
  run git init -q --bare "$D/remotes/$1.git"
  run git init -q "$H/code/$1"
  run git -C "$H/code/$1" remote add origin "https://github.com/robin-demo/$1.git"
}

new_repo shop
put "$H/code/shop" README.md <<'EOF'
# shop

The storefront: product pages, search, cart and checkout.
EOF
save "$H/code/shop" "Start the storefront"
put "$H/code/shop" src/search/search.ts <<'EOF'
import { Product } from "../catalog";

export type Query = { text: string };

export function search(products: Product[], query: Query): Product[] {
  const text = query.text.trim().toLowerCase();
  return products.filter((product) => product.name.toLowerCase().includes(text));
}
EOF
save "$H/code/shop" "Search products by name"
put "$H/code/shop" src/checkout/copy.ts <<'EOF'
export const copy = {
  pay: "Proceed to payment",
  declined: "Transaction declined (code 51)",
  empty: "Your basket contains no items",
  shipping: "Shipping costs are calculated at a later stage",
};
EOF
save "$H/code/shop" "Add the checkout copy"
put "$H/code/shop" src/cart/cart.ts <<'EOF2'
export type CartItem = { productId: string; size?: string; quantity: number };

export class Cart {
  private items: CartItem[] = [];

  add(item: CartItem) {
    const same = this.items.find((line) => line.productId === item.productId && line.size === item.size);
    if (same) same.quantity += item.quantity;
    else this.items.push(item);
  }

  remove(productId: string) {
    this.items = this.items.filter((line) => line.productId !== productId);
  }

  get lines(): CartItem[] {
    return this.items;
  }
}
EOF2
save "$H/code/shop" "Keep a cart in memory for the session"
run git -C "$H/code/shop" push -qu origin main

new_repo api
put "$H/code/api" README.md <<'EOF'
# api

Orders, invoices and API keys for the shop.
EOF
save "$H/code/api" "Start the API"
put "$H/code/api" app/invoices/vat.py <<'EOF'
from decimal import Decimal, ROUND_HALF_UP

CENT = Decimal("0.01")


def line_vat(line):
    return (line.net * line.rate).quantize(CENT, ROUND_HALF_UP)


def invoice_vat(invoice):
    return sum(line_vat(line) for line in invoice.lines)
EOF
save "$H/code/api" "Add VAT to invoices"
put "$H/code/api" app/middleware/log.py <<'EOF'
import logging

log = logging.getLogger("requests")


def log_request(request, response):
    log.info("%s %s %s key=%s", request.method, request.path, response.status, request.api_key.id)
EOF
save "$H/code/api" "Log every request"
run git -C "$H/code/api" push -qu origin main

( "$D/env.sh" herdr server > "$D/server.log" 2>&1 & echo $! > "$D/server.pid" )
for _ in $(seq 1 50); do [ -S "$H/.config/herdr/herdr.sock" ] && break; sleep 0.2; done
socket=$(demo_socket)
[ "$socket" = "$H/.config/herdr/herdr.sock" ] || { echo "herdr answered from $socket, not the demo socket; stopping" >&2; exit 1; }
[ "$scene" = update-and-restarts ] && run herdr plugin link "$install" >/dev/null

# kebbie tells the user from a Claude session by the process tree, so the user's own steps run in a pane of the demo herdr.
as_user() {
  local pane done=$D/as-user.done
  rm -f "$done"
  pane=$(run herdr workspace create --cwd "$1" --label seed --no-focus | jq -r .result.root_pane.pane_id)
  shift
  sleep 1
  run herdr pane run "$pane" "$(printf '%q ' "$@"); touch $done" >/dev/null
  for _ in $(seq 1 100); do [ -f "$done" ] && break; sleep 0.2; done
  run herdr workspace close "${pane%%:*}" >/dev/null
}

screen() { mkdir -p "$D/screens"; cat > "$D/screens/$1"; }
tee "$D/lead.screen" <<'EOF' | screen lead
> what needs me?

● Going on:
  - rate-limit: counting requests per key; the limiter is next.
  - checkout-copy: rules ✓, ready; I'd push it.
Needs you:
  - search-filters: how should the price filter work? Answer 1, fixed ranges.
  - vat-rounding: check the totals in its preview.

EOF
screen wt1.shop <<'EOF'
> Add price and size filters to the search page

● The filters are in place for size. For price there are two ways to go, so I put the question on the board.

EOF
screen wt2.shop <<'EOF'
> Rewrite the checkout copy in plain words

● Done: four strings rewritten, tests pass, ready to push.

EOF
screen wt1.api <<'EOF'
> Rate-limit the public API per key

● Reading the request log middleware to hook the limiter in.

EOF
screen wt2.api <<'EOF'
> Round VAT once per invoice instead of per line

● The new rounding is in; the preview shows old and new totals side by side for you to check.

EOF

cd "$H/code/shop"
run kebbie lead >/dev/null
run kebbie start search-filters "Add price and size filters to the search page" --no-focus >/dev/null
run kebbie start checkout-copy "Rewrite the checkout copy in plain words" --no-focus >/dev/null
cd "$H/code/api"
run kebbie start rate-limit "Rate-limit the public API per key" --no-focus >/dev/null
run kebbie start vat-rounding "Round VAT once per invoice instead of per line" --no-focus >/dev/null
if [ "$scene" = demo ]; then
  screen wt3.api <<'EOF'
> Tag every request with an ID in the log

● Done: every request gets an ID, sent back as X-Request-Id and logged with the line.

EOF
  screen wt3.shop <<'EOF'
> Lazy-load the product page's images

● The gallery loads each image as it scrolls into view; the first one loads at once.

EOF
  screen wt4.api <<'EOF'
> Refund an invoice in part or in full

● Steps listed: the refunds table, the refund, then a credit note per refund. All three are committed.

EOF
  run kebbie start request-ids "Tag every request with an ID in the log" --no-focus >/dev/null
  run kebbie start refunds "Refund an invoice in part or in full" --no-focus >/dev/null
  cd "$H/code/shop"
  run kebbie start product-images "Lazy-load the product page's images" --no-focus >/dev/null
fi

put "$H/code/wt1.shop" src/search/filters.ts <<'EOF'
import { Product } from "../catalog";

export type Filters = { sizes: string[] };

export function bySize(products: Product[], filters: Filters): Product[] {
  if (filters.sizes.length === 0) return products;
  return products.filter((product) => product.sizes.some((size) => filters.sizes.includes(size)));
}
EOF
save "$H/code/wt1.shop" "Filter search results by size" "The search page had no way to narrow a long result list. Shoppers can now tick one or more sizes and see only products that come in them."
put "$H/code/wt1.shop" src/search/price.ts <<'EOF'
export const priceRanges = [
  { label: "Under 25", max: 25 },
  { label: "25 to 50", min: 25, max: 50 },
];
EOF

put "$H/code/wt2.shop" src/checkout/copy.ts <<'EOF'
export const copy = {
  pay: "Pay now",
  declined: "Your card was declined. Try another card or contact your bank.",
  empty: "Your basket is empty",
  shipping: "Shipping costs are calculated at a later stage",
};
EOF
save "$H/code/wt2.shop" "Rewrite the checkout copy in plain words" "The checkout spoke in bank codes and stiff phrases, so shoppers stalled at the last step. Pay now, an empty basket and a declined card now read as plain sentences that say what to do."
put "$H/code/wt2.shop" src/checkout/copy.ts <<'EOF'
export const copy = {
  pay: "Pay now",
  declined: "Your card was declined. Try another card or contact your bank.",
  empty: "Your basket is empty",
  shipping: "You see the shipping cost before you pay",
};
EOF
save "$H/code/wt2.shop" "Say when the shipping cost is shown" "\"Calculated at a later stage\" left shoppers guessing whether more costs would appear after paying. The line now says they see the shipping cost before they pay."

put "$H/code/wt1.api" app/middleware/limit.py <<'EOF'
import time
from collections import defaultdict

WINDOW = 60
counts = defaultdict(list)


def over_limit(key, limit):
    now = time.monotonic()
    counts[key.id] = [at for at in counts[key.id] if now - at < WINDOW]
    counts[key.id].append(now)
    return len(counts[key.id]) > limit
EOF
save "$H/code/wt1.api" "Count requests per API key" "The public API has no limit, so one key can starve the others. Each key now keeps its request times for the last minute, the count the limiter middleware will check."

cd "$H/code/wt2.api" && run kebbie steps "Round VAT once per invoice and rate" "Preview old and new VAT per invoice" >/dev/null
put "$H/code/wt2.api" app/invoices/vat.py <<'EOF'
from collections import defaultdict
from decimal import Decimal, ROUND_HALF_UP

CENT = Decimal("0.01")


def invoice_vat(invoice):
    net_by_rate = defaultdict(Decimal)
    for line in invoice.lines:
        net_by_rate[line.rate] += line.net
    return sum((net * rate).quantize(CENT, ROUND_HALF_UP) for rate, net in net_by_rate.items())
EOF
save "$H/code/wt2.api" "Round VAT once per invoice and rate" "Rounding each line drifted from the total the accountant computes by up to a cent per line. VAT is now summed per rate across the invoice and rounded once per rate."
put "$H/code/wt2.api" app/invoices/preview.py <<'EOF'
from app.invoices.vat import invoice_vat


def preview(invoices, old_vat):
    for invoice in invoices:
        old, new = old_vat(invoice), invoice_vat(invoice)
        if old != new:
            yield invoice.number, old, new
EOF
save "$H/code/wt2.api" "Preview old and new VAT per invoice" "Changing the rounding changes totals on invoices already sent. The preview lists every invoice whose VAT differs, with the old and the new amount, to check before the change goes out."
echo working > "$D/modes/wt1.api"

cd "$H/code/wt1.api" && run kebbie report --state working --next "nothing yet" "Counting requests per key; the limiter middleware is next" >/dev/null
cd "$H/code/wt2.api" && run kebbie report --state needs-you --next "check the totals in the preview" "VAT is rounded once per invoice; the preview shows old and new totals" >/dev/null
cat > "$H/code/wt2.shop/card.md" <<EOF
The checkout speaks plainly: Pay now, a declined card says what to do, and the shipping line says when you see the cost.

![the checkout with a declined card]($D/shots/checkout-copy.png)

Confirm: the declined message reads right to you.
EOF
cd "$H/code/wt2.shop" && run kebbie report --state ready --card card.md --next "push it" "Checkout copy rewritten in plain words; tests pass" >/dev/null
cd "$H/code/wt1.shop" && run kebbie tell rate-limit "search will call the API once per keystroke; leave room for that in the limit" >/dev/null
cd "$H/code/wt1.shop" && run kebbie ask \
  --option "Fixed ranges: under 25, 25 to 50, 50 to 100, over 100" \
  --option "Slider: any min and max, updates as you drag" \
  --option "Both: ranges first, a slider behind more" \
  --recommend 1 "How should the price filter work?" >/dev/null

if [ "$scene" = demo ]; then
  put "$H/code/wt3.api" app/middleware/request_id.py <<'EOF'
import uuid


def request_id(request, response):
    request.id = request.headers.get("X-Request-Id") or uuid.uuid4().hex
    response.headers["X-Request-Id"] = request.id
EOF
  put "$H/code/wt3.api" app/middleware/log.py <<'EOF'
import logging

log = logging.getLogger("requests")


def log_request(request, response):
    log.info("%s %s %s key=%s id=%s", request.method, request.path, response.status, request.api_key.id, request.id)
EOF
  save "$H/code/wt3.api" "Tag every request with an ID in the log" "A support question about one failed call meant searching the log by time and key. Each request now carries an ID, taken from X-Request-Id or made up, sent back in the response and written on its log line."
  cd "$H/code/wt3.api" && run kebbie report --state ready --next "push it" "Every request has an ID, in its response and on its log line" >/dev/null

  put "$H/code/wt3.shop" src/product/gallery.ts <<'EOF'
export function gallery(images: string[]): string {
  return images
    .map((src, index) => `<img src="${src}" loading="${index === 0 ? "eager" : "lazy"}" decoding="async">`)
    .join("");
}
EOF
  put "$H/code/wt3.shop" src/search/search.ts <<'EOF'
import { Product } from "../catalog";

export type Query = { text: string };

export function search(products: Product[], query: Query): Product[] {
  const needle = query.text.trim().toLowerCase();
  return products.filter((product) => product.name.toLowerCase().includes(needle));
}
EOF
  save "$H/code/wt3.shop" "Lazy-load the product page's images" "A product page with twenty photos fetched them all before the first was shown, slow on a phone. The first image now loads at once and the rest as they scroll into view."
  cat > "$D/verdicts/product-images" <<'EOF'
verdict: changes

- src/search/search.ts:6 — commit {sha} also renames the search text to `needle`, a change of its own beside the lazy-loaded images; rule 2, one logical change per commit. Move the rename into a commit of its own.
EOF
  cd "$H/code/wt3.shop" && run kebbie report --state ready --next "push it" "Product images load as they scroll into view" >/dev/null

  cd "$H/code/wt4.api" && run kebbie steps "Add the refunds table" "Refund an invoice in part or in full" "Write a credit note for each refund" >/dev/null
  put "$H/code/wt4.api" app/db/migrations/0007_refunds.py <<'EOF'
def up(db):
    db.create_table("refunds", invoice_id="int", amount="decimal", reason="text", created_at="timestamp")
EOF
  save "$H/code/wt4.api" "Add the refunds table" "Refunds need a home before an invoice can carry one. The table keeps each refund's invoice, amount, reason and time, and nothing reads it yet."
  put "$H/code/wt4.api" app/invoices/refund.py <<'EOF'
from decimal import Decimal


def refund(invoice, amount: Decimal, reason: str):
    if amount <= 0 or amount > invoice.total - invoice.refunded:
        raise ValueError("a refund is more than nothing and at most what is left on the invoice")
    return invoice.refunds.create(amount=amount, reason=reason)
EOF
  save "$H/code/wt4.api" "Refund an invoice in part or in full" "Support refunded by hand in the payment provider, so the books never saw it. An invoice now takes a refund of any amount up to what is left on it, with a reason."
  put "$H/code/wt4.api" app/invoices/credit_note.py <<'EOF'
def credit_note(refund):
    return {"invoice": refund.invoice.number, "amount": -refund.amount, "reason": refund.reason}
EOF
  save "$H/code/wt4.api" "Write a credit note for each refund" "The accountant books a refund from its credit note. Each refund now gets one, a negative line against the invoice it refunds."
  as_user "$H/code/api" kebbie accept refunds "$(git -C "$H/code/wt4.api" rev-parse --short HEAD~2)"
  cd "$H/code/wt4.api" && run kebbie report --state ready --next "nothing" "Refunds in part or in full, each with a credit note" >/dev/null
  cd "$H/code/api" && run kebbie review refunds >/dev/null

  as_user "$H/code/shop" kebbie start price-alerts "Email a shopper when a saved product drops in price" --after search-filters --why "it reuses the price ranges search-filters adds" --no-focus
  as_user "$H/code/api" kebbie start invoice-archive "Move invoices older than seven years to cold storage" --at "tomorrow 06:00" --why "it locks the invoices table, so it runs before the shop opens" --no-focus

  as_user "$H/code/shop" kebbie issue add --kind bug --title "Every task's Claude is asked to append the vault log when it stops" "The vault plugin's Stop hook runs in task sessions too, so each task ends its turn writing a repo log line that only the lead should write."
  as_user "$H/code/shop" kebbie issue add --kind bug --title "herdr shows the old kebbie version after an update" "After kebbie update the plugin list in herdr still names the version before, though kebbie version prints the new one."
  python3 - "$H/.local/state/kebbie/issues" "$(git -C "$repo_root" rev-parse HEAD)" "$(git -C "$repo_root" show HEAD:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)" "$(date +%s)" <<'EOF'
import json, pathlib, sys
issues, head, version, now = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4])
fixes = {"Every task's Claude is asked to append the vault log when it stops": "8f21848", "herdr shows the old kebbie version after an update": "d387bac"}
for path in issues.glob("*.md"):
    front, body = path.read_text().split("\n+++\n", 1)
    lines = front.splitlines()
    title = next(json.loads(line.split(" = ", 1)[1]) for line in lines if line.startswith("title = "))
    if title not in fixes:
        continue
    kept = {"status": "fixed", "version": "0.26.0", "commit": "afcf5e0"}
    lines = [f"{line.split(' = ')[0]} = {json.dumps(kept[line.split(' = ')[0]])}" if line.split(" = ")[0] in kept else line for line in lines]
    lines += [f"rechecked = {json.dumps(f'{head} fixed {now - 600} {version}')}", f"history = [{json.dumps(f'fixed in {version} ({fixes[title]}), checked automatically')}]"]
    path.write_text("\n".join(lines) + "\n+++\n" + body)
EOF
fi
if [ "$scene" != demo ]; then
  cd "$H/code/shop"
  run kebbie backlog add --priority high "Wishlist — save products for later from the product page (shop)" >/dev/null
  run kebbie backlog add --priority medium "Order export — CSV of orders per month for the accountant (api)" >/dev/null
  run kebbie backlog add --priority medium "Gift cards — sell and redeem fixed-value cards (shop)" >/dev/null
  run kebbie backlog add --priority low "Slow query log — flag queries over 200 ms (api)" >/dev/null
fi
if [ "$scene" = push-and-card ]; then
  as_user "$H/code/shop" kebbie start free-shipping "Show in the checkout how far the basket is from free shipping" --after checkout-copy --why "it adds a line to the copy checkout-copy rewrites" --no-focus
fi

checked() { for _ in $(seq 1 120); do grep -qs "approved\.\|break the rules" "$H/.local/state/kebbie/rules/$1.log" && return 0; sleep 0.5; done; echo "the rules check of $1 gave no verdict" >&2; exit 1; }
checked checkout-copy@shop
for commit in HEAD~1 HEAD; do as_user "$H/code/shop" kebbie accept checkout-copy "$(git -C "$H/code/wt2.shop" rev-parse --short "$commit")"; done
if [ "$scene" = demo ]; then
  checked request-ids@api
  as_user "$H/code/api" kebbie stage request-ids
  checked product-images@shop
  for _ in $(seq 1 60); do grep -q "rules check of your commits" "$D/screens/wt3.shop" && break; sleep 0.5; done
  printf '%s\n' "" "● The rules check is right: the rename in src/search/search.ts is a change of its own. Splitting it into a second commit." >> "$D/screens/wt3.shop"
  cat > "$D/lead.screen" <<'EOF'
> what needs me?

● Going on:
  - rate-limit: counting requests per key; the limiter is next.
  - refunds: in review, as it moves money; push it once the reviewer passes it.
  - checkout-copy: rules ✓, ready; I'd push it.
  - request-ids: on local main to try before you push it.
  - product-images: broke rule 2; its Claude splits the commit.
  - devbox has the order export.
Needs you:
  - search-filters: how should the price filter work? Answer 1, fixed ranges.
  - vat-rounding: check the totals in its preview.

EOF
fi

now=$(date +%s)
printf '{"rate_limits":{"five_hour":{"used_percentage":34,"resets_at":%s},"seven_day":{"used_percentage":61,"resets_at":%s}}}' $((now + 9000)) $((now + 300000)) | run kebbie statusline


# The lead's chat starts clean once kebbie has typed the seeded tasks' messages into it.
for _ in $(seq 1 60); do grep -q "checkout-copy@shop is ready" "$D/screens/lead" && break; sleep 0.5; done
sleep 1
screen lead < "$D/lead.screen"

# The stand-ins react only once seeding is done, so the messages seeding sends do not set them off.
cp "$repo_root"/demo/react/* "$D/react/"

echo "demo world ready in $D"
