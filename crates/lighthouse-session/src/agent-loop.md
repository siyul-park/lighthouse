1. `check`.
2. `fix` the groups whose `fix` is `safe` and runs.
3. For the others, `review_show` the finding, edit with its brief, `check` again.
4. An `unsafe` fix that runs (every fix by a model is) is previewed first: `fix` the group, read the diffs, then run the `apply` of a preview to write exactly that fix. Passing `noModels` asks no model.
