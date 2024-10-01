# Metrics

Anchor comes pre-built with a suite of metrics for developers or users to monitor the health
and performance of their node.

They must be enabled at runtime using the `--metrics` CLI flag.

## Usage

In order to run a metrics server, `docker` is required to be installed.

Once docker is installed, a metrics server can be run locally via the following steps:

1. Start an anchor node with `$ anchor --metrics`
    - The `--metrics` flag is required for metrics.
1. Move into the metrics directory `$ cd metrics`.
1. Bring the environment up with `$ docker-compose up --build -d`.
1. Ensure that Prometheus can access your Anchor node by ensuring it is in
   the `UP` state at [http://localhost:9090/targets](http://localhost:9090/targets).
1. Browse to [http://localhost:3000](http://localhost:3000)
    - Username: `admin`
    - Password: `changeme`
1. Import some dashboards from the `metrics/dashboards` directory in this repo:
    - In the Grafana UI, go to `Dashboards` -> `Manage` -> `Import` -> `Upload .json file`.
    - The `Summary.json` dashboard is a good place to start.

## Dashboards

A suite of dashboards can be found in `metrics/dashboard` directory. The Anchor team will
frequently update these dashboards as new metrics are introduced.

We welcome Pull Requests for any users wishing to add their dashboards to this repository for
others to share.

## Scrape Targets

Prometheus periodically reads the `metrics/scrape-targets/scrape-targets.json` file. This
file tells Prometheus which endpoints to collect data from. The current file is setup to read
from Anchor on its default metrics port. You can add additional endpoints if you want to collect
metrics from other servers.

An example is Lighthouse. You can collect metrics from Anchor and Lighthouse simultaneously if
they are both running. We have an example file `scrape-targets-lighthouse.json` which allows this.
You can replace the `scrape-targets.json` file with the contents of
`scrape-targets-lighthouse.json` if you wish to collect metrics from Anchor and Lighthouse
simultaneously.

## Hosting Publicly

By default Prometheus and Grafana will only bind to localhost (127.0.0.1), in
order to protect you from accidentally exposing them to the public internet. If
you would like to change this you must edit the `http_addr` in `metrics/grafana/grafana.ini`.
