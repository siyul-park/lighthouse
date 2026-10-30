"use strict";

function helper(x) {
  return x + 1;
}

function Widget(name) {
  this.name = name;
}

Widget.prototype.render = function render() {
  return helper(this.name.length);
};

Widget.create = function create(name) {
  return new Widget(name);
};

exports.helper = helper;
exports.twice = function (x) {
  return helper(helper(x));
};
exports.Widget = Widget;
