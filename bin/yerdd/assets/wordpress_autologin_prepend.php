<?php
/**
 * Yerd's one-click WordPress admin login prepend script.
 *
 * Only ever loaded via a per-request `auto_prepend_file` PHP-FPM override
 * that yerd-proxy adds after validating a single-use login token - never
 * written into any site's own files, never reachable on an ordinary request.
 * If it does run, it either logs the request in and redirects to wp-admin,
 * or - if this site's own configured URL doesn't match the host and scheme
 * it's being served on - does nothing at all and lets the original request
 * continue completely normally.
 */

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

$admins = get_users([
    'role'    => 'administrator',
    'number'  => 1,
    'orderby' => 'ID',
    'order'   => 'ASC',
]);
if (!empty($admins)) {
    wp_set_auth_cookie($admins[0]->ID);
    wp_set_current_user($admins[0]->ID);
    do_action('wp_login', $admins[0]->user_login, $admins[0]);
}
wp_safe_redirect(admin_url());
exit;
