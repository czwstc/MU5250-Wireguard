#!/bin/sh
set -eu
sh /data/local/tmp/stop_open_u60_listener.sh dashboard-uhttpd 1F90
sleep 1
docroot=/data/www
if [ -L /data/www.current ]; then docroot=$(readlink -f /data/www.current); fi
test -d "$docroot"
# Release archives can carry epoch mtimes. uhttpd uses them in cache validators,
# so a browser may get 304 for an older HTML entry point after an upgrade.
# Refresh only the entry point; content-hashed assets retain their cache identity.
touch "$docroot/index.html"
trap '' HUP
nohup /data/bin/dashboard-uhttpd -f -h "$docroot" -p 0.0.0.0:8080 -D >/tmp/dashboard-uhttpd.log 2>&1 </dev/null &
echo $! > /var/run/dashboard-uhttpd.pid
