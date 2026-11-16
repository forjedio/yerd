<?php
if (!isset($_SERVER['YERD_PUBLIC_URL'])) {
    return;
}

/**
 * Yerd's WordPress public-URL prepend script for shared (tunnelled) sites.
 *
 * Only ever loaded via a per-request `auto_prepend_file` PHP-FPM override
 * that yerd-proxy adds to requests arriving through a Cloudflare Tunnel for a
 * WordPress site. `auto_prepend_file` is sent through `PHP_VALUE`, which
 * PHP-FPM does not reliably reset on a reused worker, so the gate above
 * (the paired `YERD_PUBLIC_URL` FastCGI param, sent only on tunnelled
 * requests) runs first and makes a leaked invocation a no-op.
 *
 * It does not load WordPress and touches no site files. It pre-seeds the
 * global `$wp_filter` array, which `WP_Hook::build_preinitialized_hooks`
 * turns into real hooks when WordPress boots:
 *
 *  - `option_home` / `option_siteurl` return the public URL, so links and
 *    canonical redirects use the public host instead of the local `.test` one;
 *  - `pre_update_option_home` / `pre_update_option_siteurl` return the old
 *    value, so saving Settings > General through the tunnel never writes the
 *    public URL into `wp_options`.
 *
 * Both run at `PHP_INT_MAX`, after `_config_wp_home` (priority 10), so they
 * also win over `WP_HOME` / `WP_SITEURL` constants.
 */

$yerd_public_url = rtrim((string) $_SERVER['YERD_PUBLIC_URL'], '/');
$yerd_public_url_filter = static function () use ($yerd_public_url) {
    return $yerd_public_url;
};
$yerd_keep_stored_option = static function ($value, $old_value) {
    return $old_value;
};

if (!isset($GLOBALS['wp_filter']) || !is_array($GLOBALS['wp_filter'])) {
    $GLOBALS['wp_filter'] = [];
}
foreach (['option_home', 'option_siteurl'] as $yerd_hook) {
    $GLOBALS['wp_filter'][$yerd_hook][PHP_INT_MAX][] = [
        'function' => $yerd_public_url_filter,
        'accepted_args' => 1,
    ];
}
foreach (['pre_update_option_home', 'pre_update_option_siteurl'] as $yerd_hook) {
    $GLOBALS['wp_filter'][$yerd_hook][PHP_INT_MAX][] = [
        'function' => $yerd_keep_stored_option,
        'accepted_args' => 2,
    ];
}

unset($yerd_public_url, $yerd_public_url_filter, $yerd_keep_stored_option, $yerd_hook);
