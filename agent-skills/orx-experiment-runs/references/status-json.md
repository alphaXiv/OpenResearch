# A run's status as JSON

For scripts, `orx exp status EXP --run RUN --json` returns `experimentId`,
`runId`, `status`, the current `stage` name, `progress` (`done`, `of`, `unit`,
`whole` as a fraction), `latest` signal values by name, `estimate`, and
`flags` in raise order (`id`, `state`, `text`). The estimate has remaining
milliseconds (`loMs`, `midMs`, `hiMs`), the Unix timestamp in milliseconds
when it was measured (`at`), `pending`, `floor`, and `untimed` stage names.
Unknown fields are null, latest values and flags are empty before data arrives.

Use a flag's `id` with `orx exp flag RUN --flag ID`. A flag in `kept` or
`stopped` state is settled.
