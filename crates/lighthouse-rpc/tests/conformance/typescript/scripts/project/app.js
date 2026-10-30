'use strict'

var app = (exports = module.exports = {});

app.init = function init() {
  this.parent = null;
  this.defaults();
};

app.defaults = function defaults() {
  this.mountpath = '/';
};

app.path = function path() {
  return this.parent ? this.parent.path() + this.mountpath : '';
};
