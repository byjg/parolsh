# Prints, NUL-separated, what bash-completion offers for the last word of the
# command line in $1. Parolsh runs it with `bash -c` in its current directory,
# for the arguments of `!command` lines. Prints nothing when the command has
# no completion function.

for file in /usr/share/bash-completion/bash_completion \
    /usr/local/share/bash-completion/bash_completion \
    /opt/homebrew/share/bash-completion/bash_completion \
    /etc/bash_completion; do
    if [[ -r $file ]]; then
        . "$file"
        break
    fi
done

COMP_LINE=$1
COMP_POINT=${#COMP_LINE}
# Without -r, a backslash keeps the next character in the word (My\ Files).
read -a COMP_WORDS <<< "$COMP_LINE"
[[ $COMP_LINE == *[[:space:]] ]] && COMP_WORDS+=("")
COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
command=${COMP_WORDS[0]}

# Completions are loaded on demand: _comp_load from bash-completion 2.12,
# __load_completion before.
if declare -F _comp_load > /dev/null; then
    _comp_load -- "$command"
elif declare -F __load_completion > /dev/null; then
    __load_completion "$command"
fi

spec=$(complete -p -- "$command" 2> /dev/null) || exit 0
[[ $spec =~ \ -F\ ([^ ]+) ]] || exit 0
COMPREPLY=()
"${BASH_REMATCH[1]}" "$command" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD - 1]}" > /dev/null 2>&1
((${#COMPREPLY[@]})) && printf '%s\0' "${COMPREPLY[@]}"
exit 0
