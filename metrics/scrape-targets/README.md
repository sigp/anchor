The `scrape-targets.json` file is periodically read by Prometheus and specifies
the targets it should watch.

If you want to add a node to the metrics service, do it here.

If you are running a lighthouse node on its default port and also want to collect metrics from
lighthouse, you can replace `scrape-targets.json` by `scrape-targets-lighthouse.json`.
