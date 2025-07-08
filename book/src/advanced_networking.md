# Advanced Networking

Anchor's networking stack is closely based on Lighthouse's. We refer to
[Lighthouse's page on Advanced Networking](https://lighthouse-book.sigmaprime.io/advanced_networking.html),
but want to outline several important differences:

- Currently, Anchor does not support UPnP.
- Anchor uses ports 12001 (UDP), 13001 (TCP), and 9101 (UDP) by default.
- Anchor does not yet support ENR auto-update - we therefore recommend manually setting publicly reachable ports via the
  `--enr*-port` CLI parameters to advertise your node as reachable on the network.
