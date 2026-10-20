#!/bin/bash
# Builds a throwaway kelpie world under $KDEMO_DIR: its own HOME, herdr server, repos and a stand-in claude.
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
D=${KDEMO_DIR:-$(cd /tmp && pwd -P)/kelpie-demo}
H=$D/home

case "$D" in /tmp/*|/private/tmp/*) ;; *) echo "KDEMO_DIR must be under /tmp" >&2; exit 1 ;; esac

demo_socket() { "$D/env.sh" herdr status --json 2>/dev/null | jq -r .server.socket; }

stop_demo() {
  [ -x "$D/env.sh" ] || return 0
  if [ "$(demo_socket)" = "$H/.config/herdr/herdr.sock" ]; then "$D/env.sh" herdr server stop >/dev/null 2>&1 || true; fi
  local pid; pid=$(cat "$D/server.pid" 2>/dev/null || true)
  if [ -n "$pid" ] && ps -p "$pid" -o command= | grep -q '^herdr server'; then kill "$pid"; fi
}

stop_demo
[ "${1:-}" = --stop ] && exit 0
scene=${1:-demo}
case "$scene" in demo) ;; *) [ -f "$repo_root/demo/clips/$scene.tape" ] || { echo "no scene $scene: demo or a name from demo/clips" >&2; exit 1; } ;; esac
rm -rf "$D"
mkdir -p "$D/bin" "$D/remotes" "$D/screens" "$D/modes" "$D/react" "$D/shots" "$H/code" "$H/.config/herdr" "$H/.config/kelpie" "$H/.local/state" "$H/.local/share" "$H/.cache"

install=$H/.local/share/kelpie/current
target=$(dirname "$D")/kelpie-demo-target
cargo_env=
demo_path=$D/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
if [ "$scene" = update-and-restarts ]; then
  # kelpie installed as herdr installs it, a checkout at a release tag, with a newer release on its own origin to update to.
  install=$H/.local/share/herdr/plugins/kelpie
  version=$(git -C "$repo_root" show HEAD:Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
  next=${version%.*}.$((${version##*.} + 1))
  git clone -q --bare --single-branch --no-tags "$repo_root" "$D/remotes/kelpie.git"
  git -C "$D/remotes/kelpie.git" tag "v$version" HEAD
  git clone -q "$D/remotes/kelpie.git" "$D/kelpie-next"
  perl -0pi -e "s/^version = \"\Q$version\E\"/version = \"$next\"/m" "$D/kelpie-next/Cargo.toml" "$D/kelpie-next/herdr-plugin.toml"
  perl -0pi -e "s/(name = \"kelpie\"\nversion = \")\Q$version\E/\${1}$next/" "$D/kelpie-next/Cargo.lock"
  perl -pi -e "s/\"version\": \"\Q$version\E\"/\"version\": \"$next\"/" "$D/kelpie-next/.claude-plugin/plugin.json"
  git -C "$D/kelpie-next" -c user.name="Robin Demo" -c user.email=robin@example.com commit -qam "kelpie $next"
  git -C "$D/kelpie-next" tag "v$next"
  git -C "$D/kelpie-next" push -q origin HEAD "v$next"
  git clone -q "$D/remotes/kelpie.git" "$install"
  git -C "$install" checkout -q --detach "v$version"
  cp -R "$target" "$install/target"
  cargo build --release --quiet --manifest-path "$install/Cargo.toml"
  mkdir -p "$H/.local/share/kelpie"
  ln -s "$install" "$H/.local/share/kelpie/current"
  demo_path=$D/bin:$(dirname "$(command -v cargo)"):/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
  cargo_env="RUSTUP_HOME=${RUSTUP_HOME:-$HOME/.rustup} CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}"
else
  mkdir -p "$install/target/release"
  git -C "$repo_root" archive HEAD | tar -x -C "$install"
  cargo build --release --quiet --manifest-path "$install/Cargo.toml" --target-dir "$target"
  cp "$target/release/kelpie" "$install/target/release/kelpie"
fi
ln -s "$install/target/release/kelpie" "$D/bin/kelpie"
cp "$repo_root/demo/bin/claude" "$D/bin/claude.py"
python=$(python3 -c 'import os, sys; print(os.path.realpath(sys.executable))')
app="$(dirname "$(dirname "$python")")/Resources/Python.app/Contents/MacOS/Python"
[ -x "$app" ] && python=$app
# kelpie tells the lead and a task by their process, a claude with its role's --settings, so the stand-in runs under that name; macOS's python3 stub would rename it, hence the app binary.
printf '#!/bin/bash\nexec -a claude %q %q "$@"\n' "$python" "$D/bin/claude.py" > "$D/bin/claude"
chmod +x "$D/bin/claude"

vault_env=
[ "$scene" = areas-page ] && vault_env="VAULT_PATH=$H/vault"
cat > "$D/env.sh" <<EOF
#!/bin/sh
exec env -i HOME=$H PATH=$demo_path \\
  TERM=\${TERM:-xterm-256color} COLORTERM=truecolor LANG=en_US.UTF-8 SHELL=/bin/zsh USER=demo KDEMO_DIR=$D \\
  XDG_CONFIG_HOME=$H/.config XDG_STATE_HOME=$H/.local/state XDG_DATA_HOME=$H/.local/share XDG_CACHE_HOME=$H/.cache \\
  KELPIE_STATE_DIR=$H/.local/state/kelpie KELPIE_CONFIG=$H/.config/kelpie/config.toml KELPIE_REPOS_DIR=$H/.config/kelpie/repos \\
  KELPIE_BIN=$install/target/release/kelpie KELPIE_CLAUDE_BIN=$D/bin/claude KELPIE_REPO_DIRS=$H/code KELPIE_IMAGES=halfblocks \\
  HERDR_SOCKET_PATH=$H/.config/herdr/herdr.sock $vault_env $cargo_env \\
  "\$@"
EOF
chmod +x "$D/env.sh"
run() { "$D/env.sh" "$@"; }

if [ "$scene" = areas-page ]; then
  mkdir -p "$H/vault" "$H/.cache/notes-vault-mcp"
  index=$(python3 -c 'import hashlib, sys; print(hashlib.sha1(sys.argv[1].encode()).hexdigest()[:12])' "$H/vault")
  python3 "$repo_root/demo/vault-index.py" "$H/.cache/notes-vault-mcp/$index.sqlite" "$H"
fi

chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
[ -x "$chrome" ] || chrome=$(command -v chromium || command -v google-chrome || true)
[ -n "$chrome" ] || { echo "no Chrome to render the card images with" >&2; exit 1; }
for page in card:saved-carts checkout:checkout-copy; do
  "$chrome" --headless=new --disable-gpu --hide-scrollbars --no-first-run --user-data-dir="$D/chrome" \
    --window-size=900,544 --screenshot="$D/shots/${page#*:}.png" "file://$repo_root/demo/${page%%:*}.html" >/dev/null 2>&1
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
command = "kelpie lead"
description = "kelpie: go to the lead, or back"

[[keys.command]]
key = "prefix+t"
type = "popup"
command = "kelpie board --overlay"
width = "85%"
height = "85%"
description = "kelpie board"
EOF
restart_auto=false
[ "$scene" = update-and-restarts ] && restart_auto=true
cat > "$H/.config/kelpie/config.toml" <<EOF
[restart]
auto = $restart_auto
EOF

put() { mkdir -p "$(dirname "$1/$2")"; cat > "$1/$2"; }
# The board reads the worktrees while they are seeded, so a commit retries past its index lock.
save() { for _ in 1 2 3 4 5; do run git -C "$1" add -A 2>/dev/null && run git -C "$1" commit -qm "$2" 2>/dev/null && return 0; sleep 0.5; done; run git -C "$1" commit -qm "$2"; }

new_repo() {
  run git init -q --bare "$D/remotes/$1.git"
  run git init -q "$H/code/$1"
  run git -C "$H/code/$1" remote add origin "$D/remotes/$1.git"
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

# kelpie tells the user from a Claude session by the process tree, so the user's own steps run in a pane of the demo herdr.
as_user() {
  local pane done=$D/as-user.done
  rm -f "$done"
  pane=$(run herdr workspace create --cwd "$1" --label seed --no-focus | jq -r .result.root_pane.pane_id)
  shift
  sleep 1
  run herdr pane run "$pane" "$*; touch $done" >/dev/null
  for _ in $(seq 1 100); do [ -f "$done" ] && break; sleep 0.2; done
  run herdr workspace close "${pane%%:*}" >/dev/null
}

screen() { mkdir -p "$D/screens"; cat > "$D/screens/$1"; }
tee "$D/lead.screen" <<'EOF' | screen lead
> what needs me?

● Two tasks need you. search-filters asks how the price filter should work; I recommend fixed ranges. vat-rounding has a preview of the new totals for you to check. checkout-copy is approved and ready to push, rate-limit is still working.

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
run kelpie lead >/dev/null
run kelpie start search-filters "Add price and size filters to the search page" --no-focus >/dev/null
run kelpie start checkout-copy "Rewrite the checkout copy in plain words" --no-focus >/dev/null
cd "$H/code/api"
run kelpie start rate-limit "Rate-limit the public API per key" --no-focus >/dev/null
run kelpie start vat-rounding "Round VAT once per invoice instead of per line" --no-focus >/dev/null

put "$H/code/wt1.shop" src/search/filters.ts <<'EOF'
import { Product } from "../catalog";

export type Filters = { sizes: string[] };

export function bySize(products: Product[], filters: Filters): Product[] {
  if (filters.sizes.length === 0) return products;
  return products.filter((product) => product.sizes.some((size) => filters.sizes.includes(size)));
}
EOF
save "$H/code/wt1.shop" "Filter search results by size"
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
save "$H/code/wt2.shop" "Rewrite the checkout copy in plain words"
put "$H/code/wt2.shop" src/checkout/copy.ts <<'EOF'
export const copy = {
  pay: "Pay now",
  declined: "Your card was declined. Try another card or contact your bank.",
  empty: "Your basket is empty",
  shipping: "You see the shipping cost before you pay",
};
EOF
save "$H/code/wt2.shop" "Say when the shipping cost is shown"

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
save "$H/code/wt1.api" "Count requests per API key"

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
save "$H/code/wt2.api" "Round VAT once per invoice and rate"
put "$H/code/wt2.api" app/invoices/preview.py <<'EOF'
from app.invoices.vat import invoice_vat


def preview(invoices, old_vat):
    for invoice in invoices:
        old, new = old_vat(invoice), invoice_vat(invoice)
        if old != new:
            yield invoice.number, old, new
EOF
save "$H/code/wt2.api" "Preview old and new VAT per invoice"
echo working > "$D/modes/wt1.api"

cd "$H/code/wt1.api" && run kelpie report --state working --next "nothing yet" "Counting requests per key; the limiter middleware is next" >/dev/null
cd "$H/code/wt2.api" && run kelpie report --state needs-you --next "check the totals in the preview" "VAT is rounded once per invoice; the preview shows old and new totals" >/dev/null
cat > "$H/code/wt2.shop/card.md" <<EOF
The checkout speaks plainly: Pay now, a declined card says what to do, and the shipping line says when you see the cost.

![the checkout with a declined card]($D/shots/checkout-copy.png)

Confirm: the declined message reads right to you.
EOF
cd "$H/code/wt2.shop" && run kelpie report --state ready --card card.md --next "push it" "Checkout copy rewritten in plain words; tests pass" >/dev/null
as_user "$H/code/shop" kelpie approve checkout-copy
cd "$H/code/wt1.shop" && run kelpie tell rate-limit "search will call the API once per keystroke; leave room for that in the limit" >/dev/null
cd "$H/code/wt1.shop" && run kelpie ask \
  --option "Fixed ranges: under 25, 25 to 50, 50 to 100, over 100" \
  --option "Slider: any min and max, updates as you drag" \
  --option "Both: ranges first, a slider behind more" \
  --recommend 1 "How should the price filter work?" >/dev/null

cd "$H/code/shop"
run kelpie backlog add --priority high "Wishlist — save products for later from the product page (shop)" >/dev/null
run kelpie backlog add --priority medium "Order export — CSV of orders per month for the accountant (api)" >/dev/null
run kelpie backlog add --priority medium "Gift cards — sell and redeem fixed-value cards (shop)" >/dev/null
run kelpie backlog add --priority low "Slow query log — flag queries over 200 ms (api)" >/dev/null

now=$(date +%s)
printf '{"rate_limits":{"five_hour":{"used_percentage":34,"resets_at":%s},"seven_day":{"used_percentage":61,"resets_at":%s}}}' $((now + 9000)) $((now + 300000)) | run kelpie statusline


# The lead's chat starts clean once kelpie has typed the seeded tasks' messages into it.
for _ in $(seq 1 60); do grep -q "checkout-copy@shop is ready" "$D/screens/lead" && break; sleep 0.5; done
sleep 1
screen lead < "$D/lead.screen"

# The stand-ins react only once seeding is done, so the messages seeding sends do not set them off.
cp "$repo_root"/demo/react/* "$D/react/"

echo "demo world ready in $D"
