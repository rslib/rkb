#!/bin/sh
set -e
grep -q "test.sh" .git/hooks/pre-commit
before=$(git rev-parse HEAD)
echo "# check $$" >> README.check && git add README.check && git commit -qm staged >/dev/null 2>&1
echo "# more" >> README.check
git commit -qam check >/dev/null 2>&1
test "$(git rev-list --count "$before"..HEAD)" = 2
