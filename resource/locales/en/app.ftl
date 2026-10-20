# The Updates settings group (src/settings.rs). Row titles are sentence case.
check_automatically = Check for updates automatically
frequency = Check frequency
interval_daily = Daily
interval_weekly = Weekly
interval_monthly = Monthly
auto_download = Download updates automatically
last_checked = Last checked
never = Never
just_now = Just now
minutes_ago = { $n ->
    [one] { $n } minute ago
   *[other] { $n } minutes ago
}
hours_ago = { $n ->
    [one] { $n } hour ago
   *[other] { $n } hours ago
}
days_ago = { $n ->
    [one] { $n } day ago
   *[other] { $n } days ago
}
status = Status
check_now = Check now
download_update = Download update
quit_and_install = Quit and install
managed = Updates are managed by the store or package manager that installed this app.
status_idle = Not checked yet.
status_checking = Checking for updates…
status_current = This app is up to date.
status_available = An update is available.
status_missing = No downloaded update was found.
status_unconfigured = This build has no update signing key, so it cannot check for updates.
status_invalid = The update could not be verified. This app has not been changed.
status_preparing = Downloading and verifying the update…
status_ready = The update is ready. Quit to install it and reopen the app.
status_failed = The update could not be prepared. This app has not been changed.
