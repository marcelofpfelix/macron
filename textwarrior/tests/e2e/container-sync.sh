#!/bin/sh
set -eu

fail() {
  echo "e2e: $*" >&2
  exit 1
}

assert_file_contains() {
  file=$1
  pattern=$2
  grep -F "$pattern" "$file" >/dev/null 2>&1 || fail "expected $file to contain: $pattern"
}

write_mock_task() {
  root=$1
  export_json=$2
  mock_task="$root/mock-task"
  export_file="$root/task-export.json"
  import_file="$root/task-imported.json"
  args_file="$root/task-args.txt"

  printf '%s\n' "$export_json" >"$export_file"
  cat >"$mock_task" <<EOF
#!/bin/sh
set -eu
printf '%s\n' "\$*" >> "$args_file"
last=''
for arg in "\$@"; do
  last="\$arg"
done
if [ "\$last" = 'export' ]; then
  cat "$export_file"
  exit 0
fi
if [ "\${1:-}" = 'import' ]; then
  cp "\$2" "$import_file"
  exit 0
fi
echo "unexpected mock task args: \$*" >&2
exit 1
EOF
  chmod +x "$mock_task"
  printf '%s\n' "$mock_task"
}

run_full_sync_e2e() {
  root=$(mktemp -d)
  todo_dir="$root/tasks"
  mkdir -p "$todo_dir"
  task_command=$(write_mock_task "$root" '[{"uuid":"550e8400-e29b-41d4-a716-446655440000","status":"pending","description":"Fix bridge","annotations":[{"description":"textwarrior id:p-1"},{"description":"textwarrior source:personal.todo"}]}]')
  config="$root/config.toml"
  cat >"$config" <<EOF
todo_dir = "$todo_dir"
task_command = "$task_command"

[[files]]
name = "personal"
path = "personal-todo.md"
export_txt = "personal-todo.txt"
EOF

  textwarrior --config "$config" sync

  assert_file_contains "$todo_dir/personal-todo.md" "Fix bridge"
  assert_file_contains "$todo_dir/personal-todo.md" "id:p-1"
  assert_file_contains "$todo_dir/personal-todo.txt" "Fix bridge"
  assert_file_contains "$root/task-imported.json" '"description": "Fix bridge"'
  assert_file_contains "$todo_dir/.textwarrior/state.json" '"id:p-1"'
  assert_file_contains "$root/task-args.txt" "export"
  grep -F "import " "$root/task-args.txt" >/dev/null 2>&1 || fail "expected import call"
}

run_profile_state_e2e() {
  root=$(mktemp -d)
  todo_dir="$root/tasks"
  mkdir -p "$todo_dir"
  cat >"$todo_dir/personal-todo.md" <<EOF
- [ ] Old personal id:p-1 source:personal.todo
EOF
  cat >"$todo_dir/work-todo.md" <<EOF
- [ ] Keep work id:w-1 source:work.todo
EOF
  task_command=$(write_mock_task "$root" '[{"status":"pending","description":"Updated personal","annotations":[{"description":"textwarrior id:p-1"},{"description":"textwarrior source:personal.todo"}]}]')
  config="$root/config.toml"
  cat >"$config" <<EOF
todo_dir = "$todo_dir"
task_command = "$task_command"

[[files]]
name = "personal"
path = "personal-todo.md"
export_txt = "personal-todo.txt"

[[files]]
name = "work"
path = "work-todo.md"
export_txt = "work-todo.txt"

[[profiles]]
name = "personal"
files = ["personal"]
task_filter = ["+personal"]
EOF

  textwarrior --config "$config" sync
  textwarrior --config "$config" sync --profile personal

  assert_file_contains "$todo_dir/personal-todo.md" "Updated personal"
  assert_file_contains "$todo_dir/work-todo.md" "Keep work"
  assert_file_contains "$todo_dir/.textwarrior/state.json" '"id:p-1"'
  assert_file_contains "$todo_dir/.textwarrior/state.json" '"id:w-1"'
  assert_file_contains "$root/task-args.txt" "+personal export"
}

textwarrior --version
run_full_sync_e2e
run_profile_state_e2e
echo "container e2e passed"
