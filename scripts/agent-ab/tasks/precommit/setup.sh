#!/bin/sh
set -e
chmod +x test.sh
git init -q && git config user.name t && git config user.email t@t
printf '#!/bin/sh\nexec ./test.sh\n' > .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit
git add -A && git commit -qm init
echo "# change" >> test.sh
