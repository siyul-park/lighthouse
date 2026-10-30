'use strict'

var path = require('node:path')
  , sep = require('node:path').sep;

function usedAtLoad(name) {
  return path.join(name, sep);
}

function neverUsed() {
  return 0;
}

for (const name of ['a', 'b']) {
  console.info(usedAtLoad(name));
}
