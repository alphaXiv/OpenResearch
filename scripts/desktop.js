import('./dev-slot.mjs')
  .then(({ desktop }) => desktop())
  .catch((error) => {
    console.error(`desktop: ${error.message}`)
    process.exitCode = 1
  })
