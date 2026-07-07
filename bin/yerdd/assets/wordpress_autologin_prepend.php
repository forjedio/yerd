<?php
/**
 * Yerd's one-click WordPress admin login prepend script.
 *
 * Only ever loaded via a per-request `auto_prepend_file` PHP-FPM override
 * that yerd-proxy adds after validating a single-use login token - never
 * written into any site's own files, never reachable on an ordinary request
 * that legitimately carries the paired `YERD_LOGIN_USER` FastCGI param (see
 * the first guard below - required because `auto_prepend_file` itself can
 * outlive the request that set it on a reused PHP-FPM worker). If it does
 * run, it either logs the request in (as the site's configured admin, or
 * the earliest-created administrator if none was configured) and redirects
 * to wp-admin, or - if this site's own configured URL doesn't match the
 * host and scheme it's being served on - does nothing at all and lets the
 * original request continue completely normally.
 */

// This file's own execution gate: `YERD_LOGIN_USER` is a plain FastCGI
// param (not a PHP ini setting), always sent by yerd-proxy on the one
// request that validated a token - but `auto_prepend_file` itself is a
// `PHP_INI_PERDIR` setting sent via the FastCGI `PHP_VALUE` mechanism, which
// PHP-FPM does not reliably reset between requests on a reused worker: a
// later request on the same worker can still have this file prepended even
// though yerd-proxy correctly stopped sending the override. Requiring
// `YERD_LOGIN_USER` to be *set* (not just checking `??`) is what actually
// gates this script's effect to genuinely-validated requests; without it,
// this was observed to auto-login and redirect on every subsequent
// `/wp-admin` hit on the same worker, producing an infinite redirect loop.
// Checked first, before anything else, so a leaked/stale invocation costs
// nothing beyond this one comparison.
if (!isset($_SERVER['YERD_LOGIN_USER'])) {
    return;
}

$wp_load = rtrim($_SERVER['DOCUMENT_ROOT'] ?? '', '/') . '/wp-load.php';
if (!is_file($wp_load)) {
    return;
}
require $wp_load;

// The guard that makes this safe for any WordPress install, not just ones
// yerd itself created: only proceed if this site's own configured URL - host
// *and* scheme - agrees with how it's actually being served. A scheme
// mismatch (e.g. a parked site whose siteurl is still http:// while yerd now
// serves it over https://) is just as unsafe to proceed on as a host
// mismatch: wp_set_auth_cookie()'s cookie flavour and admin_url()'s scheme
// both follow the *current* request, not the stored siteurl, so a mismatch
// here is exactly the kind of stale/incorrect configuration this guard
// exists to decline on rather than paper over.
$configured_host = wp_parse_url(home_url(), PHP_URL_HOST);
$configured_scheme = wp_parse_url(home_url(), PHP_URL_SCHEME);
$requested_scheme = is_ssl() ? 'https' : 'http';
$requested_host = wp_parse_url($requested_scheme . '://' . ($_SERVER['HTTP_HOST'] ?? ''), PHP_URL_HOST);
if (!$configured_host || strcasecmp($configured_host, (string) $requested_host) !== 0) {
    return;
}
if (!$configured_scheme || strcasecmp($configured_scheme, $requested_scheme) !== 0) {
    return;
}

// The target admin, resolved once at mint time (see
// wordpress_login::mint_wordpress_login_token) and passed through as a
// custom FastCGI param - "" means no specific user was configured, so fall
// back to the earliest-created administrator. A configured user who's since
// been deleted or demoted falls back the same way, never hard-failing.
$target_login = $_SERVER['YERD_LOGIN_USER'];
$admin = $target_login !== '' ? get_user_by('login', $target_login) : false;
if ($admin && !in_array('administrator', (array) $admin->roles, true)) {
    $admin = false;
}
if (!$admin) {
    $admins = get_users([
        'role'    => 'administrator',
        'number'  => 1,
        'orderby' => 'ID',
        'order'   => 'ASC',
    ]);
    $admin = $admins[0] ?? null;
}
if ($admin) {
    wp_set_auth_cookie($admin->ID);
    wp_set_current_user($admin->ID);
    do_action('wp_login', $admin->user_login, $admin);
}
wp_safe_redirect(admin_url());
exit;
