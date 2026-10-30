const lib = require("./lib");
const { twice } = require("./lib");

module.exports = {
  run: function run() {
    return twice(1) + lib.helper(2);
  },
  VERSION: "1",
};
