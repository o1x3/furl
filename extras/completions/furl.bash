# bash completion for furl / furls
# Install: source this file, or copy to /etc/bash_completion.d/

_furl() {
    local cur prev opts
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    opts="--json --form --multipart --boundary --raw --compress \
--pretty --style --unsorted --sorted --response-charset --response-mime \
--format-options --print --headers --meta --body --verbose --all --stream \
--output --download --continue --quiet --session --session-read-only \
--auth --auth-type --ignore-netrc --offline --proxy --follow --max-redirects \
--max-headers --timeout --check-status --path-as-is --chunked --verify --ssl \
--ciphers --cert --cert-key --cert-key-pass --ignore-stdin --help --manual \
--version --traceback --default-scheme --debug \
-j -f -x -s -p -h -m -b -v -S -o -d -c -q -a -A -F -I -P"

    case "$prev" in
        --pretty)
            COMPREPLY=( $(compgen -W "all colors format none" -- "$cur") ); return 0 ;;
        --auth-type|-A)
            COMPREPLY=( $(compgen -W "basic bearer digest" -- "$cur") ); return 0 ;;
        --ssl)
            COMPREPLY=( $(compgen -W "tls1.2 tls1.3" -- "$cur") ); return 0 ;;
        --output|-o|--cert|--cert-key)
            COMPREPLY=( $(compgen -f -- "$cur") ); return 0 ;;
    esac

    if [[ "$cur" == -* ]]; then
        COMPREPLY=( $(compgen -W "$opts" -- "$cur") )
    fi
    return 0
}
complete -F _furl furl
complete -F _furl furls
