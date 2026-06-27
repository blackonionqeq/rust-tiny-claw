#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
env_file="${FEISHU_ENV_FILE:-$repo_root/.env.feishu}"
template_file="${FEISHU_NGINX_TEMPLATE:-$repo_root/deploy/nginx/tiny-claw-feishu.conf.template}"
output_file="${FEISHU_NGINX_OUTPUT:-$repo_root/deploy/nginx/tiny-claw-feishu.conf}"

if [ ! -f "$env_file" ]; then
    echo "missing env file: $env_file" >&2
    echo "copy .env.feishu.example to .env.feishu and fill it first" >&2
    exit 1
fi

if [ ! -f "$template_file" ]; then
    echo "missing nginx template: $template_file" >&2
    exit 1
fi

read_env_value() {
    key="$1"
    sed -n "s/^[[:space:]]*$key[[:space:]]*=[[:space:]]*//p" "$env_file" \
        | tail -n 1 \
        | sed 's/[[:space:]]*#.*$//' \
        | sed 's/\r$//' \
        | sed 's/^"//; s/"$//; s/^'\''//; s/'\''$//'
}

FEISHU_PUBLIC_HOST="${FEISHU_PUBLIC_HOST:-$(read_env_value FEISHU_PUBLIC_HOST)}"
FEISHU_CALLBACK_PORT="${FEISHU_CALLBACK_PORT:-$(read_env_value FEISHU_CALLBACK_PORT)}"
FEISHU_UPSTREAM_HOST="${FEISHU_UPSTREAM_HOST:-$(read_env_value FEISHU_UPSTREAM_HOST)}"
FEISHU_TLS_CERT="${FEISHU_TLS_CERT:-$(read_env_value FEISHU_TLS_CERT)}"
FEISHU_TLS_KEY="${FEISHU_TLS_KEY:-$(read_env_value FEISHU_TLS_KEY)}"

: "${FEISHU_PUBLIC_HOST:?missing FEISHU_PUBLIC_HOST in $env_file}"
: "${FEISHU_CALLBACK_PORT:?missing FEISHU_CALLBACK_PORT in $env_file}"

FEISHU_UPSTREAM_HOST="${FEISHU_UPSTREAM_HOST:-127.0.0.1}"
FEISHU_TLS_CERT="${FEISHU_TLS_CERT:-/etc/letsencrypt/live/$FEISHU_PUBLIC_HOST/fullchain.pem}"
FEISHU_TLS_KEY="${FEISHU_TLS_KEY:-/etc/letsencrypt/live/$FEISHU_PUBLIC_HOST/privkey.pem}"

escape_sed_replacement() {
    printf '%s' "$1" | sed 's/[\/&]/\\&/g'
}

mkdir -p "$(dirname -- "$output_file")"

sed \
    -e "s/__FEISHU_PUBLIC_HOST__/$(escape_sed_replacement "$FEISHU_PUBLIC_HOST")/g" \
    -e "s/__FEISHU_UPSTREAM_HOST__/$(escape_sed_replacement "$FEISHU_UPSTREAM_HOST")/g" \
    -e "s/__FEISHU_CALLBACK_PORT__/$(escape_sed_replacement "$FEISHU_CALLBACK_PORT")/g" \
    -e "s/__FEISHU_TLS_CERT__/$(escape_sed_replacement "$FEISHU_TLS_CERT")/g" \
    -e "s/__FEISHU_TLS_KEY__/$(escape_sed_replacement "$FEISHU_TLS_KEY")/g" \
    "$template_file" > "$output_file"

echo "rendered nginx config: $output_file"
echo "Feishu callback URL: https://$FEISHU_PUBLIC_HOST/feishu/events"
