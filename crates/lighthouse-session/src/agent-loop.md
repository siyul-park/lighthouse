1. `check`.
2. `fix` the groups whose `fix` is `safe`.
3. For the others, `review_show` the finding, edit with its brief, `check` again.
4. Use `fix` with a model only if the project bound one and you want it to run: an `unsafe` fix comes back as a preview whose `apply` call writes exactly that fix, and `noModels` asks none.
