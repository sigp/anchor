# Anchor Metrics

[![metrics.png](https://i.postimg.cc/Jh7rxtgp/metrics.png)](https://postimg.cc/4YMRN4Xc)

Here we provides a `docker-compose` environment which scrapes metrics from Anchor
nodes using Prometheus and presents them in a browser-based Grafana GUI.

For in-depth details on running metrics, please see the [anchor
book](https://anchor-book.sigmaprime.io).

## Basic Usage

1. Start an anchor node with `$ anchor --metrics`
    - The `--metrics` flag is required for metrics.
1. Move in to the `metrics` directory `$ cd metrics`.
1. Bring the environment up with `$ docker-compose up --build`.
1. Ensure that Prometheus can access your Anchor node by ensuring it is in
   the `UP` state at [http://localhost:9090/targets](http://localhost:9090/targets).
1. Browse to [http://localhost:3000](http://localhost:3000)
    - Username: `admin`
    - Password: `changeme`
1. Import some dashboards from the `dashboards` directory in this repo:
    - In the Grafana UI, go to `Dashboards` -> `Manage` -> `Import` -> `Upload .json file`.
    - The `Summary.json` dashboard is a good place to start.

## Hosting Publicly

By default Prometheus and Grafana will only bind to localhost (127.0.0.1), in
order to protect you from accidentally exposing them to the public internet. If
you would like to change this you must edit the `http_addr` in `grafana.ini`.

## Maintenance and Contributing

The Anchor team has a hosted version of this stack where we do the
majority of the monitoring. The dashboards in this repo are not guaranteed to
be kept updated as we add/modify metrics to Anchor. If you're having
problems, please reach out and we can update the dashboards here.

Feel free to create your own dashboards, export them and submit them here as
PRs.
