//// [mod1.js]
Object.defineProperty(exports, "thing", {
    value: 42,
    writable: true
});
Object.defineProperty(exports, "readonlyProp", {
    value: "Smith",
    writable: false
});
Object.defineProperty(exports, "rwAccessors", {
    get: function get() {
        return 98122;
    },
    set: function set(_) {}
});
Object.defineProperty(exports, "readonlyAccessor", {
    get: function get() {
        return 21.75;
    }
});
Object.defineProperty(exports, "setonlyAccessor", {
    /** @param {string} str */ set: function set(str) {
        this.rwAccessors = Number(str);
    }
});
//// [mod2.js]
Object.defineProperty(module.exports, "thing", {
    value: "yes",
    writable: true
});
Object.defineProperty(module.exports, "readonlyProp", {
    value: "Smith",
    writable: false
});
Object.defineProperty(module.exports, "rwAccessors", {
    get: function get() {
        return 98122;
    },
    set: function set(_) {}
});
Object.defineProperty(module.exports, "readonlyAccessor", {
    get: function get() {
        return 21.75;
    }
});
Object.defineProperty(module.exports, "setonlyAccessor", {
    /** @param {string} str */ set: function set(str) {
        this.rwAccessors = Number(str);
    }
});
//// [index.js]
/**
 * @type {number}
 */ var q = require("./mod1").thing;
/**
 * @type {string}
 */ var u = require("./mod2").thing;
//// [validator.ts]
//! 
//!   x Import assignment cannot be used when targeting ECMAScript modules. Consider using `import * as ns from "mod"`, `import {a} from "mod"`, `import d from "mod"`, or another module format instead.
//!    ,----
//!  3 | import m1 = require("./mod1");
//!    : ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
//!    `----
//! 
//!   x Import assignment cannot be used when targeting ECMAScript modules. Consider using `import * as ns from "mod"`, `import {a} from "mod"`, `import d from "mod"`, or another module format instead.
//!     ,----
//!  23 | import m2 = require("./mod2");
//!     : ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
//!     `----
