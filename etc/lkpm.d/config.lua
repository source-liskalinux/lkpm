-- ------------------------------------------------------
-- ::: [ LISKA PACKAGE MANAGER SYSTEM CONFIGURATION ] :::
-- ------------------------------------------------------
--
-- This file configures behavior rules for lkpm.
--
-- > Param: [ install_root | db_path | cache_path | parallel_operation
--          | blocked_packages | no_update ]
--
-- > arch                : The architecture of the system (e.g x86_64).
-- > install_root        : The root directory where packages will be installed.
-- > db_path             : The path to the lkpm default database file.
-- > cache_path          : The path to the lkpm default cache directory.
-- > parallel_operation  : The number of parallel operations to run at once.
-- > blocked_packages    : A list of package names to block from installation.
-- > no_update           : A list of package names to block from updates.
--
-- > NOTE FOR BLOCKED PACKAGES: Use "blocked_packages" to list package names to block. 
--                              Trailing '*' is a prefix wildcard (for example, 'foo*' 
--                              matches 'fooiso'). Entries without '*' are matched exactly 
--                              (for example, 'foo' matches only that name). Be careful,
--                              a single '*' will match everything!
--
-- > NOTE FOR NO_UPDATE PACKAGES: Use "no_update" to list package names to block from updates. 
--                                Trailing '*' is a prefix wildcard (for example, 'foo*' matches 
--                                'fooiso'). Entries without '*' are matched exactly (for example, 
--                                'foo' matches only that name). Be careful, a single '*' will
--                                match everything!
--
-- > NOTE: Changes take effect the next time "lkpm" is called 
--         (no daemon restart).
-- --------------------------------------------------------------------------


arch = "x86_64"
install_root = "/"
db_path = "/var/lib/lkpm"
cache_path = "/var/cache/lkpm"
parallel_operation = 4
--blocked_packages = {
--    "foo-pkg",
--    "bar-pkg",
--    "baz-pkg*"
--}
--no_update = {
--    "foo-pkg",
--    "bar-pkg",
--    "baz-pkg*"
--}