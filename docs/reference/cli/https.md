# HTTPS

`secure` promotes a parked site to a linked entry and serves it over HTTPS using the local certificate authority; `unsecure` stops serving it over HTTPS. See [HTTPS & Certificates](../../guide/https).

| Command | Description | Example |
| --- | --- | --- |
| `yerd secure <NAME>` | Serve a site over HTTPS (promotes a parked site to a linked entry). | `yerd secure blog` |
| `yerd unsecure <NAME>` | Stop serving a site over HTTPS. | `yerd unsecure blog` |

```sh
yerd secure blog      # https://blog.test is now served with a trusted cert
yerd unsecure blog    # back to http only
```

::: tip
For the browser to trust the certificate, the local CA must be installed in your OS trust store. Run `sudo yerd elevate trust` once (see [Elevation](./elevation)).
:::
