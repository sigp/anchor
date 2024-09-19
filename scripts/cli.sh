#! /usr/bin/env bash

# IMPORTANT
# This script should NOT be run directly.
# Run `make cli` or `make cli-local` from the root of the repository instead.

set -e

# A function to generate formatted .md files
write_to_file() {
    local cmd="$1"
    local file="$2"
    local program="$3"

    # We need to add the header and the backticks to create the code block.
    printf "# %s\n\n\`\`\`\n%s\n\`\`\`" "$program" "$cmd" > "$file"

    # Adjust the width of the help text and append to the end of file
    sed -i -e '$a\'$'\n''\n''<style> .content main {max-width:88%;} </style>' "$file"
}

CMD=./target/release/ssv

# Store all help strings in variables.
general_cli=$($CMD --help)

general=./help_general.md

# create .md files
write_to_file "$general_cli" "$general" "SSV General Commands"

#input 1 = $1 = files; input 2 = $2 = new files
files=(./book/src/help_general.md ./book/src/help_bn.md ./book/src/help_vc.md ./book/src/help_vm.md ./book/src/help_vm_create.md ./book/src/help_vm_import.md ./book/src/help_vm_move.md)
new_files=($general $bn $vc $vm $vm_create $vm_import $vm_move)

# function to check
check() {
    local file="$1"
    local new_file="$2"

    if [[ -f $file ]]; then # check for existence of file
        diff=$(diff $file $new_file || :)
    else
        cp $new_file $file
        changes=true
        echo "$file is not found, it has just been created"
    fi

    if [[ -z $diff ]]; then # check for difference
        : # do nothing
    else
        cp $new_file $file
        changes=true
        echo "$file has been updated"
    fi
}

# define changes as false
changes=false
# call check function to check for each help file
check ${files[0]} ${new_files[0]}
check ${files[1]} ${new_files[1]}
check ${files[2]} ${new_files[2]}
check ${files[3]} ${new_files[3]}
check ${files[4]} ${new_files[4]}
check ${files[5]} ${new_files[5]}
check ${files[6]} ${new_files[6]}

# remove help files
rm -f help_general.md help_bn.md help_vc.md help_am.md help_vm.md help_vm_create.md help_vm_import.md help_vm_move.md

# only exit at the very end
if [[ $changes == true ]]; then
    echo "Exiting with error to indicate changes occurred. To fix, run 'make cli-local' or 'make cli' and commit the changes."
    exit 1
else
    echo "CLI help texts are up to date."
    exit 0
fi
