// The API of Acrobat's JavaScript that forms use, for the scripts of a PDF. Rust calls
// `__handle(json)` with what happened and gets back what the scripts did; `__ask(json)` waits for
// the answer to a dialog. Written from Adobe's "JavaScript for Acrobat API Reference". See
// "JavaScript" in docs/design/26-pdf.md.
(function () {
  "use strict";

  const G = globalThis;
  const ask = G.__ask;
  const parse = JSON.parse;
  const stringify = JSON.stringify;
  const globalEval = eval;

  const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August",
    "September", "October", "November", "December"];
  const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
  const DATE_FORMATS = ["m/d", "m/d/yy", "mm/dd/yy", "mm/yy", "d-mmm", "d-mmm-yy", "dd-mmm-yy",
    "yy-mm-dd", "mmm-yy", "mmmm-yy", "mmm d, yyyy", "mmmm d, yyyy", "m/d/yy h:MM tt",
    "m/d/yy HH:MM"];
  const TIME_FORMATS = ["HH:MM", "h:MM tt", "HH:MM:ss", "h:MM:ss tt"];
  const INVALID = "The value entered does not match the format of the field";

  let effects = [];
  const fields = [];
  const byName = new Map();
  let order = [];
  const pageScripts = new Map();
  // The fields whose values changed in this event, and whether that was other than by working
  // them out, which then works out the rest.
  let changed = new Set();
  let outside = false;
  let calculating = false;
  let source = null;
  let pageCount = 0;
  let currentPage = 0;
  let fileName = "";
  let info = {};
  let calculate = true;
  let dirty = false;
  let openAction = null;
  const timers = new Map();
  let nextTimer = 1;

  function emit(effect) {
    effects.push(effect);
  }

  function log(text) {
    emit({ t: "log", text: String(text) });
  }

  // Runs a script of the document in the global scope, where `this` is the document, as in
  // Acrobat. What it throws goes to the log; running out of time can't be caught.
  function run(code, event, what) {
    if (!code) {
      return;
    }
    const before = G.event;
    G.event = event;
    try {
      globalEval(code);
    } catch (e) {
      log(`${what}: ${e}${e && e.stack ? "\n" + e.stack : ""}`);
    } finally {
      G.event = before;
    }
  }

  function makeEvent(name, type, target, more) {
    return Object.assign({
      name, type, target, source: source || target, rc: true, value: "", change: "",
      changeEx: "", commitKey: 0, fieldFull: false, keyDown: false, modifier: false,
      shift: false, selStart: 0, selEnd: 0, willCommit: false, richValue: [], richChange: [],
      richChangeEx: [],
      targetName: target && typeof target.name === "string" ? target.name : "",
    }, more);
  }

  function alert(message) {
    G.app.alert(message, 0, 0);
  }

  function isNumberText(s) {
    return /^\s*[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?\s*$/.test(s);
  }

  function same(a, b) {
    if (Array.isArray(a) && Array.isArray(b)) {
      return a.length === b.length && a.every((x, i) => x === b[i]);
    }
    return a === b;
  }

  // Fields.

  const PROPERTIES = {
    textColor: ["G", 0], fillColor: ["T"], borderColor: ["T"], strokeColor: ["T"], textSize: 0,
    textFont: "Helvetica", alignment: "left", borderStyle: "solid", lineWidth: 1, userName: "",
    submitName: "", rotation: 0, style: "check", highlight: "none", doNotScroll: false,
    doNotSpellCheck: false, fileSelect: false, commitOnSelChange: false,
    radiosInUnison: false, defaultStyle: {}, delay: false, buttonAlignX: 50, buttonAlignY: 50,
    buttonFitBounds: false, buttonPosition: 0, buttonScaleHow: 0, buttonScaleWhen: 0,
    richText: false, richValue: [], print: true,
  };

  class Field {
    constructor(data, index) {
      this._index = index;
      this._data = data;
      this._raw = this._toRaw(data.value);
      this._default = this._toRaw(data.default);
      this._display = data.widgets.length ? data.widgets[0].display : 0;
      this._readonly = data.readonly;
      this._required = data.required;
      this._options = data.options.map((o) => o.slice());
      this._charLimit = data.charLimit || 0;
      this._properties = {};
      this._rect = data.widgets.length ? data.widgets[0].rect.slice() : [0, 0, 0, 0];
    }

    get name() { return this._data.name; }
    get type() { return this._data.type; }

    get value() {
      const raw = this._raw;
      switch (this.type) {
        case "text":
        case "combobox":
          return raw.trim() !== "" && isNumberText(raw) ? Number(raw) : raw;
        case "listbox":
          if (raw.length > 1) {
            return raw.slice();
          }
          return raw.length ? raw[0] : "";
        default:
          return raw;
      }
    }

    set value(v) {
      this._setValue(v);
    }

    get valueAsString() {
      return Array.isArray(this._raw) ? this._raw.join(",") : String(this._raw);
    }

    get defaultValue() {
      return Array.isArray(this._default) ? this._default.slice() : this._default;
    }

    set defaultValue(v) {
      this._default = this._toRaw(v);
    }

    _toRaw(v) {
      switch (this.type) {
        case "listbox":
          if (Array.isArray(v)) {
            return v.map(String);
          }
          return v === null || v === undefined || v === "" ? [] : [String(v)];
        case "checkbox":
        case "radiobutton":
        case "button":
          return v === null || v === undefined || v === "" ? "Off" : String(v);
        case "combobox":
          if (Array.isArray(v)) {
            return v.length ? String(v[0]) : "";
          }
          return v === null || v === undefined ? "" : String(v);
        default:
          if (v === null || v === undefined) {
            return "";
          }
          return Array.isArray(v) ? v.join(",") : String(v);
      }
    }

    _setValue(v) {
      const raw = this._toRaw(v);
      if (same(raw, this._raw)) {
        return;
      }
      this._raw = raw;
      changed.add(this);
      dirty = true;
      if (!calculating) {
        outside = true;
      }
    }

    get display() { return this._display; }

    set display(d) {
      d = Number(d) | 0;
      if (d < 0 || d > 3 || d === this._display) {
        return;
      }
      this._display = d;
      emit({ t: "display", f: this._index, d });
    }

    get hidden() { return this._display === 1; }
    set hidden(b) { this.display = b ? 1 : 0; }

    get readonly() { return this._readonly; }

    set readonly(b) {
      b = !!b;
      if (b !== this._readonly) {
        this._readonly = b;
        emit({ t: "readonly", f: this._index, on: b });
      }
    }

    get required() { return this._required; }
    set required(b) { this._required = !!b; }

    get page() {
      const pages = this._data.widgets.map((w) => w.page);
      return pages.length > 1 ? pages : (pages[0] ?? 0);
    }

    get rect() { return this._rect.slice(); }
    set rect(r) { this._rect = Array.from(r).map(Number); }

    get exportValues() { return this._data.states.slice(); }
    get charLimit() { return this._charLimit; }
    set charLimit(n) { this._charLimit = Number(n) | 0; }
    get comb() { return this._data.comb; }
    get multiline() { return this._data.multiline; }
    get password() { return this._data.password; }
    get editable() { return this._data.editable; }
    get multipleSelection() { return this._data.multiple; }
    get numItems() { return this._options.length; }
    get calcOrderIndex() { return order.indexOf(this._index); }

    getItemAt(i, exported) {
      if (i < 0) {
        i = this._options.length - 1;
      }
      const option = this._options[i];
      if (!option) {
        return "";
      }
      return exported === false ? option[1] : option[0];
    }

    get currentValueIndices() {
      const picked = Array.isArray(this._raw) ? this._raw : [this._raw];
      const indices = picked
        .map((v) => this._options.findIndex((o) => o[0] === v))
        .filter((i) => i >= 0);
      if (this.type === "listbox" && indices.length > 1) {
        return indices;
      }
      return indices.length ? indices[0] : -1;
    }

    set currentValueIndices(v) {
      const list = Array.isArray(v) ? v : [v];
      const picked = list.map((i) => this._options[i]).filter(Boolean).map((o) => o[0]);
      this._setValue(this.type === "listbox" ? picked : (picked[0] ?? ""));
    }

    setItems(items) {
      this._options = Array.from(items).map((item) => {
        if (Array.isArray(item)) {
          return [String(item[1] ?? item[0]), String(item[0])];
        }
        return [String(item), String(item)];
      });
    }

    clearItems() {
      this._options = [];
    }

    insertItemAt(name, exported, at) {
      const option = [String(exported ?? name), String(name)];
      const i = at === undefined || at < 0 ? 0 : Math.min(at, this._options.length);
      this._options.splice(i, 0, option);
    }

    deleteItemAt(at) {
      const i = at === undefined || at < 0 ? this._options.length - 1 : at;
      this._options.splice(i, 1);
    }

    _stateOf(widget) {
      const w = this._data.widgets[widget || 0];
      return (w && w.state) || this._data.states[0] || "Yes";
    }

    isBoxChecked(widget) {
      return this._raw !== "Off" && this._raw === this._stateOf(widget);
    }

    isDefaultChecked(widget) {
      return this._default !== "Off" && this._default === this._stateOf(widget);
    }

    checkThisBox(widget, on) {
      const state = this._stateOf(widget);
      if (on === undefined || on) {
        this._setValue(state);
      } else if (this._raw === state) {
        this._setValue("Off");
      }
    }

    defaultIsChecked(widget, on) {
      this._default = on === undefined || on ? this._stateOf(widget) : "Off";
    }

    getArray() {
      return [this];
    }

    getLock() { return null; }
    setLock() { return false; }

    setFocus() {
      emit({ t: "focus", f: this._index });
    }

    buttonGetCaption() { return ""; }
    buttonSetCaption() {}
    buttonGetIcon() { return null; }
    buttonSetIcon() {}
    buttonImportIcon() { return -1; }
    browseForFileToSubmit() {}
    setAction() {}
    signatureInfo() { return {}; }
    signatureSign() { return false; }
    signatureValidate() { return 0; }
  }

  for (const [name, start] of Object.entries(PROPERTIES)) {
    Object.defineProperty(Field.prototype, name, {
      get() { return name in this._properties ? this._properties[name] : start; },
      set(v) { this._properties[name] = v; },
    });
  }

  // What `getField` gives for a name that only the fields under it have.
  class Group {
    constructor(name, kids) {
      this.name = name;
      this._kids = kids;
    }

    get type() { return this._kids[0].type; }
    get value() { return this._kids[0].value; }
    set value(v) { for (const kid of this._kids) kid.value = v; }
    get valueAsString() { return this._kids[0].valueAsString; }
    get display() { return this._kids[0].display; }
    set display(d) { for (const kid of this._kids) kid.display = d; }
    get hidden() { return this._kids[0].hidden; }
    set hidden(b) { for (const kid of this._kids) kid.hidden = b; }
    get readonly() { return this._kids[0].readonly; }
    set readonly(b) { for (const kid of this._kids) kid.readonly = b; }
    get required() { return this._kids[0].required; }
    set required(b) { for (const kid of this._kids) kid.required = b; }
    getArray() { return this._kids.slice(); }
    setFocus() { this._kids[0].setFocus(); }
  }

  function getField(name) {
    name = String(name);
    const field = byName.get(name);
    if (field) {
      return field;
    }
    const kids = fields.filter((f) => f.name.startsWith(name + "."));
    if (kids.length) {
      return new Group(name, kids);
    }
    // "name.0" is a widget of the field.
    const widget = /^(.*)\.(\d+)$/.exec(name);
    return (widget && byName.get(widget[1])) || null;
  }

  // Working out, showing and committing values.

  function calculateNow() {
    calculating = true;
    try {
      const list = order.slice();
      for (let i = 0; i < fields.length; i++) {
        if (fields[i]._data.scripts.C && !list.includes(i)) {
          list.push(i);
        }
      }
      for (const i of list) {
        const field = fields[i];
        const code = field && field._data.scripts.C;
        if (!code) {
          continue;
        }
        const event = makeEvent("Calculate", "Field", field, { value: field.value });
        run(code, event, `the calculation of ${field.name}`);
        if (!event.rc || event.value === undefined) {
          continue;
        }
        const raw = field._toRaw(event.value);
        if (same(raw, field._raw)) {
          continue;
        }
        // What is worked out gets checked as what is typed does, as in Acrobat.
        const validate = field._data.scripts.V;
        if (validate) {
          const check = makeEvent("Validate", "Field", field, { value: raw, willCommit: true });
          run(validate, check, `the validation of ${field.name}`);
          if (!check.rc) {
            continue;
          }
        }
        field._setValue(raw);
      }
    } finally {
      calculating = false;
    }
  }

  // What shows for a field, if its script says something else than its value.
  function formatted(field) {
    const code = field._data.scripts.F;
    if (!code) {
      return null;
    }
    const event = makeEvent("Format", "Field", field, { value: field.value });
    run(code, event, `the format of ${field.name}`);
    if (!event.rc) {
      return null;
    }
    const shown = event.value === null || event.value === undefined ? "" : String(event.value);
    return shown === field.valueAsString ? null : shown;
  }

  function finish() {
    if (outside && calculate) {
      calculateNow();
    }
    outside = false;
    for (const field of changed) {
      emit({ t: "value", f: field._index, v: field._raw, s: formatted(field) });
    }
    changed = new Set();
    source = null;
  }

  function refuse(field) {
    emit({ t: "refused", f: field._index, v: field._raw, s: formatted(field) });
  }

  function commit(m) {
    const field = fields[m.f];
    if (!field) {
      return;
    }
    const scripts = field._data.scripts;
    let raw = field._toRaw(m.v);
    if (["text", "combobox", "listbox"].includes(field.type)) {
      if (scripts.K) {
        const text = Array.isArray(raw) ? raw.join(",") : raw;
        const event = makeEvent("Keystroke", "Field", field, {
          value: field.type === "listbox" ? raw : raw, willCommit: true,
          selStart: text.length, selEnd: text.length,
        });
        run(scripts.K, event, `the keystroke of ${field.name}`);
        if (!event.rc) {
          refuse(field);
          return;
        }
        raw = field._toRaw(event.value);
      }
      if (scripts.V) {
        const event = makeEvent("Validate", "Field", field, { value: raw, willCommit: true });
        run(scripts.V, event, `the validation of ${field.name}`);
        if (!event.rc) {
          refuse(field);
          return;
        }
      }
    }
    source = field;
    field._setValue(raw);
  }

  function keystroke(m) {
    const field = fields[m.f];
    const code = field && field._data.scripts.K;
    if (!code) {
      emit({ t: "typed", ok: true, text: m.value, sel: [m.selEnd, m.selEnd] });
      return;
    }
    const event = makeEvent("Keystroke", "Field", field, {
      value: m.value, change: m.change, changeEx: m.change, selStart: m.selStart,
      selEnd: m.selEnd, willCommit: false, keyDown: true,
    });
    run(code, event, `the keystroke of ${field.name}`);
    if (!event.rc) {
      emit({ t: "typed", ok: false, text: m.value, sel: [m.selStart, m.selEnd] });
      return;
    }
    const clamp = (n) => Math.max(0, Math.min(m.value.length, Number(n) | 0));
    const start = clamp(event.selStart);
    const end = Math.max(start, clamp(event.selEnd));
    const change = event.change === null || event.change === undefined ? "" : String(event.change);
    const text = m.value.slice(0, start) + change + m.value.slice(end);
    const at = start + change.length;
    emit({ t: "typed", ok: true, text, sel: [at, at] });
  }

  const MOUSE = {
    "Mouse Down": "down", "Mouse Up": "up", "Mouse Enter": "enter", "Mouse Exit": "exit",
    "Focus": "focus", "Blur": "blur",
  };

  function mouse(m) {
    const field = fields[m.f];
    const widget = field && field._data.widgets[m.w];
    const code = widget && widget.scripts[MOUSE[m.name]];
    if (!code) {
      return;
    }
    source = field;
    const event = makeEvent(m.name, "Field", field, {
      value: field.value, shift: !!m.shift, modifier: !!m.modifier,
    });
    run(code, event, `${m.name} of ${field.name}`);
  }

  function page(m) {
    if (m.open) {
      currentPage = m.n;
    }
    const scripts = pageScripts.get(m.n);
    const code = scripts && (m.open ? scripts[0] : scripts[1]);
    run(code, makeEvent(m.open ? "Open" : "Close", "Page", G), `page ${m.n + 1}`);
  }

  function sync(m) {
    for (const [i, v] of m.values) {
      const field = fields[i];
      if (!field) {
        continue;
      }
      if (m.calculate) {
        field._setValue(v);
        changed.add(field);
        outside = true;
      } else {
        field._raw = field._toRaw(v);
      }
    }
    for (const [i, d] of m.display) {
      if (fields[i]) {
        fields[i]._display = d;
      }
    }
    for (const [i, b] of m.readonly) {
      if (fields[i]) {
        fields[i]._readonly = b;
      }
    }
  }

  function init(m) {
    pageCount = m.pages;
    currentPage = m.page;
    fileName = m.file;
    info = m.info;
    m.fields.forEach((data, i) => {
      const field = new Field(data, i);
      fields.push(field);
      if (!byName.has(data.name)) {
        byName.set(data.name, field);
      }
    });
    order = m.order;
    for (const [n, open, close] of m.pageScripts) {
      pageScripts.set(n, [open, close]);
    }
    openAction = m.open;
    for (const [name, code] of m.named) {
      run(code, makeEvent("Open", "Doc", G), `the script ${name}`);
    }
    // What has no look yet gets one as it should show; the others have theirs already. The
    // functions of the document are there for it now.
    for (const field of fields) {
      if (field._data.needsLook && !changed.has(field) && field.valueAsString !== "") {
        const shown = formatted(field);
        if (shown !== null) {
          emit({ t: "look", f: field._index, s: shown });
        }
      }
    }
    dirty = false;
  }

  // What runs when it opens comes after what it looks like has gone back, as it may wait for a
  // dialog.
  function opened() {
    run(openAction, makeEvent("Open", "Doc", G), "the script that runs when opening");
    page({ n: currentPage, open: true });
  }

  // Timers.

  function addTimer(code, ms, every) {
    const id = nextTimer++;
    const wait = Math.max(0, Number(ms) || 0);
    timers.set(id, { code, wait, at: Date.now() + wait, every });
    return { _timer: id, toString() { return "[object Timer]"; } };
  }

  function removeTimer(timer) {
    timers.delete(timer && typeof timer === "object" ? timer._timer : timer);
  }

  function runTimers() {
    const now = Date.now();
    for (const [id, timer] of Array.from(timers)) {
      if (timer.at > now) {
        continue;
      }
      if (timer.every) {
        timer.at = now + Math.max(timer.wait, 10);
      } else {
        timers.delete(id);
      }
      if (typeof timer.code === "function") {
        try {
          timer.code();
        } catch (e) {
          log(`a timer: ${e}`);
        }
      } else {
        run(String(timer.code), makeEvent("Exec", "App", G), "a timer");
      }
    }
  }

  function nextDelay() {
    let next = -1;
    const now = Date.now();
    for (const timer of timers.values()) {
      const wait = Math.max(0, timer.at - now);
      if (next < 0 || wait < next) {
        next = wait;
      }
    }
    return next;
  }

  // The document, which is the global object, as `this` is the document in Acrobat.

  function define(name, get, set) {
    Object.defineProperty(G, name, { get, set: set || (() => {}), configurable: true });
  }

  function value(name, v) {
    Object.defineProperty(G, name, { value: v, writable: true, configurable: true });
  }

  function notDone(what) {
    return function () {
      log(`${what} isn't supported here`);
    };
  }

  define("numPages", () => pageCount);
  define("pageNum", () => currentPage, (n) => {
    n = Number(n) | 0;
    if (n >= 0 && n < pageCount) {
      currentPage = n;
      emit({ t: "page", n });
    }
  });
  define("numFields", () => fields.length);
  define("info", () => Object.assign({}, info, {
    Title: info.title, Author: info.author, Subject: info.subject, Keywords: info.keywords,
    Creator: info.creator, Producer: info.producer,
  }));
  for (const key of ["title", "author", "subject", "keywords", "creator", "producer"]) {
    define(key, () => info[key] ?? "");
  }
  define("documentFileName", () => fileName);
  define("path", () => "/" + fileName);
  define("URL", () => "file:///" + fileName);
  define("dirty", () => dirty, (b) => { dirty = !!b; });
  define("calculate", () => calculate, (b) => { calculate = !!b; });
  define("delay", () => false);
  define("external", () => false);
  define("filesize", () => 0);
  define("layout", () => "SinglePage");
  define("zoom", () => 100);
  define("zoomType", () => "NoVary");
  define("mouseX", () => 0);
  define("mouseY", () => 0);
  define("disclosed", () => false);
  define("bookmarkRoot", () => null);
  define("baseURL", () => "");

  value("getField", getField);
  value("getNthFieldName", (n) => (fields[n] ? fields[n].name : null));
  value("calculateNow", () => calculateNow());
  value("resetForm", (names) => {
    let list = fields;
    if (names !== undefined && names !== null) {
      const wanted = Array.isArray(names) ? names.map(String) : [String(names)];
      list = fields.filter((f) => wanted.some((n) => f.name === n || f.name.startsWith(n + ".")));
    }
    for (const field of list) {
      field._setValue(field._default);
    }
  });
  value("getURL", (url) => emit({ t: "url", url: String(url) }));
  value("print", notDone("Printing"));
  value("submitForm", notDone("Sending a form"));
  value("mailDoc", notDone("Mailing a document"));
  value("mailForm", notDone("Mailing a form"));
  value("exportAsFDF", notDone("FDF"));
  value("exportAsXFDF", notDone("XFDF"));
  value("importAnFDF", notDone("FDF"));
  value("saveAs", notDone("Saving from a script"));
  value("closeDoc", notDone("Closing from a script"));
  value("gotoNamedDest", notDone("Going to a named destination"));
  value("bringToFront", () => {});
  value("syncAnnotScan", () => {});
  value("getAnnots", () => []);
  value("getAnnot", () => null);
  value("getOCGs", () => null);
  value("addField", () => null);
  value("removeField", () => {});
  value("addScript", () => {});
  value("setAction", () => {});
  value("setPageAction", () => {});
  value("getPageNthWord", () => "");
  value("getPageNumWords", () => 0);
  value("getPageLabel", (n) => String((Number(n) | 0) + 1));
  value("getPageRotation", () => 0);
  value("getIcon", () => null);
  value("getPrintParams", () => ({}));
  value("getTemplate", () => null);
  value("scroll", () => {});

  // `app`.

  G.app = {
    alert(message, icon, type, title) {
      if (message && typeof message === "object") {
        ({ cMsg: message, nIcon: icon, nType: type, cTitle: title } = message);
      }
      const answer = parse(ask(stringify({
        t: "alert", message: String(message ?? ""), title: title ? String(title) : "",
        icon: Number(icon) | 0, buttons: Number(type) | 0,
      })));
      return typeof answer === "number" ? answer : 1;
    },
    response(question, title, start, password) {
      if (question && typeof question === "object") {
        ({ cQuestion: question, cTitle: title, cDefault: start, bPassword: password } = question);
      }
      return parse(ask(stringify({
        t: "response", message: String(question ?? ""), title: title ? String(title) : "",
        default: start === undefined || start === null ? "" : String(start),
        password: !!password,
      })));
    },
    beep() {},
    launchURL(url) {
      emit({ t: "url", url: String(url) });
    },
    setTimeOut(code, ms) { return addTimer(code, ms, false); },
    clearTimeOut(timer) { removeTimer(timer); },
    setInterval(code, ms) { return addTimer(code, ms, true); },
    clearInterval(timer) { removeTimer(timer); },
    get calculate() { return calculate; },
    set calculate(b) { calculate = !!b; },
    get activeDocs() { return [G]; },
    viewerType: "Reader",
    viewerVariation: "Reader",
    viewerVersion: 21.007,
    formsVersion: 21.007,
    platform: "UNIX",
    language: "ENU",
    focusRect: true,
    runtimeHighlight: false,
    runtimeHighlightColor: ["RGB", 0.8, 0.8, 1],
    numPlugIns: 0,
    plugIns: [],
    printerNames: [],
    toolbar: true,
    toolbarHorizontal: true,
    toolbarVertical: true,
    fs: { isFullScreen: false },
    media: {},
    monitors: [],
    thermometer: {
      begin() {}, end() {}, cancelled: false, duration: 0, value: 0, text: "",
    },
    execMenuItem(name) {
      log(`the menu item ${name} isn't supported here`);
    },
    addMenuItem() {},
    addSubMenu() {},
    addToolButton() {},
    removeToolButton() {},
    hideMenuItem() {},
    hideToolbarButton() {},
    listMenuItems() { return []; },
    listToolbarButtons() { return []; },
    goBack() {},
    goForward() {},
    mailMsg: notDone("Mailing"),
    newDoc() { return null; },
    openDoc() { return null; },
    openFDF() { return null; },
    popUpMenu() { return null; },
    popUpMenuEx() { return null; },
    browseForDoc() { return undefined; },
    getNthPlugInName() { return ""; },
    getPath() { return ""; },
    trustedFunction(f) { return f; },
    trustPropagatorFunction(f) { return f; },
    beginPriv() {},
    endPriv() {},
  };

  G.console = {
    println(text) { log(text); },
    show() {},
    hide() {},
    clear() {},
  };

  G.color = {
    transparent: ["T"], black: ["G", 0], white: ["G", 1], red: ["RGB", 1, 0, 0],
    green: ["RGB", 0, 1, 0], blue: ["RGB", 0, 0, 1], cyan: ["CMYK", 1, 0, 0, 0],
    magenta: ["CMYK", 0, 1, 0, 0], yellow: ["CMYK", 0, 0, 1, 0], dkGray: ["G", 0.25],
    gray: ["G", 0.5], ltGray: ["G", 0.75],
    convert(c, space) {
      const [from, a = 0, b = 0, d = 0, k = 0] = c;
      if (from === space || from === "T") {
        return c.slice();
      }
      let rgb;
      switch (from) {
        case "G": rgb = [a, a, a]; break;
        case "RGB": rgb = [a, b, d]; break;
        case "CMYK": rgb = [1 - Math.min(1, a + k), 1 - Math.min(1, b + k), 1 - Math.min(1, d + k)]; break;
        default: return c.slice();
      }
      switch (space) {
        case "G": return ["G", 0.3 * rgb[0] + 0.59 * rgb[1] + 0.11 * rgb[2]];
        case "RGB": return ["RGB", ...rgb];
        case "CMYK": return ["CMYK", 1 - rgb[0], 1 - rgb[1], 1 - rgb[2], 0];
        default: return c.slice();
      }
    },
    equal(a, b) {
      if (a[0] === "T" || b[0] === "T") {
        return a[0] === b[0];
      }
      const x = G.color.convert(a, "RGB");
      const y = G.color.convert(b, "RGB");
      return x.every((v, i) => i === 0 || Math.abs(v - y[i]) < 1e-6);
    },
  };

  Object.assign(G, {
    display: { visible: 0, hidden: 1, noPrint: 2, noView: 3 },
    border: { s: "solid", b: "beveled", d: "dashed", i: "inset", u: "underline" },
    style: { ch: "check", cr: "cross", di: "diamond", ci: "circle", st: "star", sq: "square" },
    font: {
      Times: "Times-Roman", TimesB: "Times-Bold", TimesI: "Times-Italic",
      TimesBI: "Times-BoldItalic", Helv: "Helvetica", HelvB: "Helvetica-Bold",
      HelvI: "Helvetica-Oblique", HelvBI: "Helvetica-BoldOblique", Cour: "Courier",
      CourB: "Courier-Bold", CourI: "Courier-Oblique", CourBI: "Courier-BoldOblique",
      Symbol: "Symbol", ZapfD: "ZapfDingbats",
    },
    highlight: { n: "none", i: "invert", p: "push", o: "outline" },
    position: { textOnly: 0, iconOnly: 1, iconTextV: 2, textIconV: 3, iconTextH: 4, textIconH: 5, overlay: 6 },
    scaleHow: { proportional: 0, anamorphic: 1 },
    scaleWhen: { always: 0, never: 1, tooBig: 2, tooSmall: 3 },
    zoomtype: {
      none: "NoVary", fitP: "FitPage", fitW: "FitWidth", fitH: "FitHeight", fitV: "FitVisibleWidth",
      pref: "Preferred", refW: "ReflowWidth",
    },
    event: makeEvent("Init", "App", G),
  });

  // `util`.

  function pad(n, width) {
    return String(n).padStart(width || 2, "0");
  }

  function printf(format, ...args) {
    let next = 0;
    return String(format).replace(/%(,[0-4])?([+ 0#]*)(\d+)?(?:\.(\d+))?([dfsx%])/g,
      (whole, separators, flags, width, precision, conversion) => {
        if (conversion === "%") {
          return "%";
        }
        const arg = args[next++];
        let out;
        if (conversion === "s") {
          out = arg === undefined || arg === null ? "" : String(arg);
          if (precision !== undefined) {
            out = out.slice(0, Number(precision));
          }
        } else if (conversion === "x") {
          out = (Math.trunc(Number(arg) || 0) >>> 0).toString(16);
        } else {
          let n = Number(arg);
          if (!isFinite(n)) {
            n = 0;
          }
          const digits = conversion === "d" ? 0 : (precision === undefined ? 6 : Number(precision));
          if (conversion === "d") {
            n = Math.trunc(n);
          }
          const fixed = Math.abs(n).toFixed(digits);
          const negative = n < 0 && /[1-9]/.test(fixed);
          let [whole, fraction] = fixed.split(".");
          const style = separators ? Number(separators[1]) : 1;
          const group = [",", "", ".", "", "'"][style];
          const point = [".", ".", ",", ",", "."][style];
          if (group) {
            whole = whole.replace(/\B(?=(\d{3})+(?!\d))/g, group);
          }
          out = fraction !== undefined ? whole + point + fraction :
            (flags.includes("#") && conversion === "f" ? whole + point : whole);
          const sign = negative ? "-" : flags.includes("+") ? "+" : flags.includes(" ") ? " " : "";
          if (width && flags.includes("0")) {
            out = out.padStart(Number(width) - sign.length, "0");
          }
          out = sign + out;
        }
        return width ? out.padStart(Number(width), " ") : out;
      });
  }

  const TOKENS = /(\\[\s\S])|(mmmm|mmm|mm|m|dddd|ddd|dd|d|yyyy|yy|HH|H|hh|h|MM|M|ss|s|tt|t)|([\s\S])/g;

  function namedFormat(format) {
    if (typeof format === "number") {
      return ["D:yyyymmddHHMMss", "yyyy.mm.dd HH:MM:ss", "m/d/yy h:MM:ss tt"][format] ?? "";
    }
    return String(format ?? "");
  }

  function printd(format, date) {
    if (!(date instanceof Date) || isNaN(date.getTime())) {
      return "";
    }
    format = namedFormat(format);
    const hours = date.getHours();
    const twelve = hours % 12 === 0 ? 12 : hours % 12;
    const parts = {
      mmmm: () => MONTHS[date.getMonth()],
      mmm: () => MONTHS[date.getMonth()].slice(0, 3),
      mm: () => pad(date.getMonth() + 1),
      m: () => String(date.getMonth() + 1),
      dddd: () => DAYS[date.getDay()],
      ddd: () => DAYS[date.getDay()].slice(0, 3),
      dd: () => pad(date.getDate()),
      d: () => String(date.getDate()),
      yyyy: () => String(date.getFullYear()),
      yy: () => pad(date.getFullYear() % 100),
      HH: () => pad(hours),
      H: () => String(hours),
      hh: () => pad(twelve),
      h: () => String(twelve),
      MM: () => pad(date.getMinutes()),
      M: () => String(date.getMinutes()),
      ss: () => pad(date.getSeconds()),
      s: () => String(date.getSeconds()),
      tt: () => (hours < 12 ? "am" : "pm"),
      t: () => (hours < 12 ? "a" : "p"),
    };
    return format.replace(TOKENS, (whole, escaped, token, other) => {
      if (escaped) {
        return escaped[1];
      }
      return token ? parts[token]() : other;
    });
  }

  function monthOf(name) {
    const start = String(name).toLowerCase().replace(/\.$/, "").slice(0, 3);
    return start.length < 3 ? -1 : MONTHS.findIndex((m) => m.toLowerCase().startsWith(start));
  }

  function escapeRegExp(s) {
    return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }

  // A date from its parts, if there is such a date.
  function dateOf(year, month, day, hour, minute, second) {
    const date = new Date(year, month, day, hour, minute, second);
    const real = date.getFullYear() === year && date.getMonth() === month &&
      date.getDate() === day && date.getHours() === hour && date.getMinutes() === minute &&
      date.getSeconds() === second;
    return real ? date : null;
  }

  function fullYear(n, digits) {
    if (digits > 2) {
      return n;
    }
    return n < 50 ? 2000 + n : 1900 + n;
  }

  function scand(format, text) {
    format = namedFormat(format);
    text = String(text ?? "");
    let pattern = "^\\s*";
    const tokens = [];
    let m;
    TOKENS.lastIndex = 0;
    while ((m = TOKENS.exec(format))) {
      if (m[1]) {
        pattern += escapeRegExp(m[1][1]);
      } else if (m[2]) {
        tokens.push(m[2]);
        switch (m[2]) {
          case "mmmm": case "mmm": case "dddd": case "ddd":
            pattern += "([A-Za-z\\u00c0-\\u024f]+\\.?)"; break;
          case "yyyy": pattern += "(\\d{4})"; break;
          case "yy": pattern += "(\\d{2})"; break;
          case "tt": pattern += "([AaPp]\\.?[Mm]?\\.?)"; break;
          case "t": pattern += "([AaPp])"; break;
          default: pattern += "(\\d{1,2})";
        }
      } else {
        pattern += /\s/.test(m[3]) ? "\\s*" : escapeRegExp(m[3]);
      }
    }
    pattern += "\\s*$";
    const found = new RegExp(pattern).exec(text);
    if (!found) {
      return null;
    }
    const now = new Date();
    let year = now.getFullYear();
    let month = 0;
    let day = 1;
    let hour = 0;
    let minute = 0;
    let second = 0;
    let pm = null;
    for (let i = 0; i < tokens.length; i++) {
      const v = found[i + 1];
      switch (tokens[i]) {
        case "mmmm": case "mmm": month = monthOf(v); if (month < 0) return null; break;
        case "mm": case "m": month = Number(v) - 1; break;
        case "dd": case "d": day = Number(v); break;
        case "yyyy": year = Number(v); break;
        case "yy": year = fullYear(Number(v), 2); break;
        case "HH": case "H": case "hh": case "h": hour = Number(v); break;
        case "MM": case "M": minute = Number(v); break;
        case "ss": case "s": second = Number(v); break;
        case "tt": case "t": pm = /^p/i.test(v); break;
      }
    }
    if (pm !== null) {
      if (hour < 1 || hour > 12) {
        return null;
      }
      hour = (hour % 12) + (pm ? 12 : 0);
    }
    return dateOf(year, month, day, hour, minute, second);
  }

  // A date that doesn't quite have the format: the numbers in the order the format has them.
  function parseDate(format, text) {
    format = namedFormat(format);
    const strict = scand(format, text);
    if (strict) {
      return strict;
    }
    const words = String(text ?? "").match(/[A-Za-zÀ-ɏ]+|\d+/g) || [];
    const numbers = [];
    let named = -1;
    let pm = null;
    for (const word of words) {
      if (/^\d+$/.test(word)) {
        numbers.push(word);
      } else if (/^(a|am|p|pm)$/i.test(word)) {
        pm = /^p/i.test(word);
      } else if (named < 0 && monthOf(word) >= 0) {
        named = monthOf(word);
      } else {
        return null;
      }
    }
    const hasDate = /[ymd]/.test(format.replace(/\\[\s\S]/g, ""));
    let order = ["y", "m", "d"].filter((k) => format.includes(k))
      .sort((a, b) => format.indexOf(a) - format.indexOf(b));
    if (!order.length) {
      order = ["m", "d", "y"];
    }
    if (named >= 0) {
      order = order.filter((k) => k !== "m");
    }
    const now = new Date();
    let [year, month, day] = [now.getFullYear(), named >= 0 ? named : 0, 1];
    // A number of four digits is the year wherever it is.
    const long = numbers.findIndex((n) => n.length === 4);
    if (hasDate && long >= 0 && order.includes("y")) {
      year = Number(numbers.splice(long, 1)[0]);
      order = order.filter((k) => k !== "y");
    }
    if (hasDate) {
      for (const key of order) {
        const n = numbers.shift();
        if (n === undefined) {
          break;
        }
        if (key === "y") {
          year = fullYear(Number(n), n.length);
        } else if (key === "m") {
          month = Number(n) - 1;
        } else {
          day = Number(n);
        }
      }
    }
    let [hour, minute, second] = numbers.map(Number).concat([0, 0, 0]);
    if (pm !== null) {
      if (hour < 1 || hour > 12) {
        return null;
      }
      hour = (hour % 12) + (pm ? 12 : 0);
    }
    if (!hasDate && words.length === 0) {
      return null;
    }
    return dateOf(year, month, day, hour, minute, second);
  }

  function printx(format, text) {
    format = String(format ?? "");
    text = String(text ?? "");
    let out = "";
    let j = 0;
    let shape = (c) => c;
    const take = (test) => {
      while (j < text.length && !test(text[j])) {
        j++;
      }
      if (j < text.length) {
        out += shape(text[j++]);
      }
    };
    for (let i = 0; i < format.length; i++) {
      const f = format[i];
      switch (f) {
        case "?": if (j < text.length) out += shape(text[j++]); break;
        case "X": take((c) => /[A-Za-z0-9]/.test(c)); break;
        case "A": take((c) => /[A-Za-z]/.test(c)); break;
        case "9": take((c) => /[0-9]/.test(c)); break;
        case "*": while (j < text.length) out += shape(text[j++]); break;
        case "\\": i++; if (i < format.length) out += format[i]; break;
        case ">": shape = (c) => c.toUpperCase(); break;
        case "<": shape = (c) => c.toLowerCase(); break;
        case "=": shape = (c) => c; break;
        default: out += f;
      }
    }
    return out;
  }

  G.util = {
    printf, printd, scand, printx,
    crackURL(url) {
      const m = /^(\w+):\/\/([^/:?#]*)(?::(\d+))?([^?#]*)(?:\?([^#]*))?(?:#(.*))?$/.exec(String(url));
      return m ? {
        cScheme: m[1], cHost: m[2], nPort: m[3] ? Number(m[3]) : undefined, cPath: m[4],
        cQuery: m[5], cFragment: m[6],
      } : {};
    },
    iconStreamFromIcon() { return null; },
    streamFromString() { return null; },
    stringFromStream() { return ""; },
  };

  // The functions of Acrobat's AForm.js that forms call by name.

  function AFMakeNumber(v) {
    if (typeof v === "number") {
      return isFinite(v) ? v : null;
    }
    if (typeof v !== "string") {
      return null;
    }
    let s = v.replace(/\s/g, "");
    if (!s) {
      return null;
    }
    const dot = s.lastIndexOf(".");
    const comma = s.lastIndexOf(",");
    if (dot >= 0 && comma >= 0) {
      const point = Math.max(dot, comma);
      s = s.slice(0, point).replace(/[.,]/g, "") + "." + s.slice(point + 1);
    } else if (comma >= 0) {
      s = s.split(",").length > 2 ? s.replace(/,/g, "") : s.replace(",", ".");
    } else if (s.split(".").length > 2) {
      s = s.replace(/\./g, "");
    }
    return isNumberText(s) ? Number(s) : null;
  }

  function AFMergeChange(event) {
    event = event || G.event;
    let v = event.value;
    v = v === null || v === undefined ? "" : String(v);
    if (event.willCommit) {
      return v;
    }
    const start = event.selStart >= 0 ? event.selStart : 0;
    const end = event.selEnd >= 0 ? event.selEnd : start;
    return v.slice(0, start) + (event.change ?? "") + v.slice(end);
  }

  function AFMakeArrayFromList(list) {
    if (typeof list === "string") {
      return list.split(",").map((s) => s.trim()).filter(Boolean);
    }
    return Array.from(list || []);
  }

  function targetName(event) {
    return event.target && event.target.name ? `[ ${event.target.name} ]` : "";
  }

  function AFNumber_Format(decimals, separators, negatives, currencyStyle, currency, prepend) {
    const event = G.event;
    const n = AFMakeNumber(event.value);
    if (n === null) {
      event.value = "";
      return;
    }
    separators = Math.min(Math.max(0, Math.floor(Number(separators) || 0)), 4);
    currency = currency ? String(currency) : "";
    const number = printf(`%,${separators}.${Math.max(0, Number(decimals) | 0)}f`, Math.abs(n));
    const shown = prepend ? currency + number : number + currency;
    const negative = n < 0 && /[1-9]/.test(number);
    if (negatives === 1 || negatives === 3) {
      event.target.textColor = negative ? ["RGB", 1, 0, 0] : ["G", 0];
    }
    if (!negative) {
      event.value = shown;
    } else if (negatives === 2 || negatives === 3) {
      event.value = `(${shown})`;
    } else if (negatives === 1) {
      event.value = shown;
    } else {
      event.value = "-" + shown;
    }
  }

  function AFNumber_Keystroke(decimals, separators) {
    const event = G.event;
    let v = AFMergeChange(event);
    if (!v) {
      return;
    }
    v = v.trim();
    const comma = separators > 1;
    let pattern;
    if (comma) {
      pattern = event.willCommit ? /^[+-]?(\d+(,\d*)?|,\d+)$/ : /^[+-]?\d*,?\d*$/;
    } else {
      pattern = event.willCommit ? /^[+-]?(\d+(\.\d*)?|\.\d+)$/ : /^[+-]?\d*\.?\d*$/;
    }
    // What was written with separators between thousands still reads when it is committed.
    const plain = event.willCommit ? v.replace(comma ? /\./g : /,/g, "") : v;
    if (!pattern.test(plain)) {
      if (event.willCommit) {
        alert(`${INVALID} ${targetName(event)}`);
      }
      event.rc = false;
      return;
    }
    if (event.willCommit) {
      event.value = comma ? plain.replace(",", ".") : plain;
    }
  }

  function AFPercent_Format(decimals, separators, prepend) {
    const event = G.event;
    const n = AFMakeNumber(event.value);
    if (n === null) {
      event.value = "";
      return;
    }
    separators = Math.min(Math.max(0, Math.floor(Number(separators) || 0)), 4);
    const number = printf(`%,${separators}.${Math.max(0, Number(decimals) | 0)}f`, n * 100);
    event.value = prepend ? "%" + number : number + "%";
  }

  function AFPercent_Keystroke(decimals, separators) {
    AFNumber_Keystroke(decimals, separators, 0, 0, "", true);
  }

  function AFDate_FormatEx(format) {
    const event = G.event;
    const v = event.value;
    if (v === "" || v === null || v === undefined) {
      return;
    }
    const date = parseDate(format, v);
    if (date) {
      event.value = printd(format, date);
    }
  }

  function AFDate_KeystrokeEx(format) {
    const event = G.event;
    if (!event.willCommit) {
      return;
    }
    const v = AFMergeChange(event);
    if (!v) {
      return;
    }
    if (!parseDate(format, v)) {
      alert("Invalid date/time: please ensure that the date/time exists. Field " +
        `${targetName(event)} should match format ${format}`);
      event.rc = false;
    }
  }

  function AFSpecial_Format(kind) {
    const event = G.event;
    const v = event.value === null || event.value === undefined ? "" : String(event.value);
    if (!v) {
      return;
    }
    const masks = ["99999", "99999-9999", null, "999-99-9999"];
    let mask = masks[kind];
    if (kind === 2) {
      mask = v.replace(/\D/g, "").length >= 10 ? "(999) 999-9999" : "999-9999";
    }
    if (mask) {
      event.value = printx(mask, v);
    }
  }

  function AFSpecial_Keystroke(kind) {
    const event = G.event;
    const v = AFMergeChange(event);
    if (!v) {
      return;
    }
    const digits = v.replace(/\D/g, "").length;
    const sizes = [[5, 5], [9, 9], [7, 10], [9, 9]][kind];
    if (!sizes) {
      return;
    }
    const allowed = kind === 2 ? /^[\d\s().+-]*$/ : /^[\d\s-]*$/;
    const fits = allowed.test(v) && digits <= sizes[1];
    const done = digits === sizes[0] || digits === sizes[1];
    if (!fits || (event.willCommit && !done)) {
      if (event.willCommit) {
        alert(`${INVALID} ${targetName(event)}`);
      }
      event.rc = false;
    }
  }

  function AFSpecial_KeystrokeEx(mask) {
    if (!mask) {
      return;
    }
    const event = G.event;
    const v = AFMergeChange(event);
    if (!v) {
      return;
    }
    const fits = (c, m) => {
      switch (m) {
        case "9": return /\d/.test(c);
        case "A": return /[A-Za-z]/.test(c);
        case "O": return /[A-Za-z0-9]/.test(c);
        case "X": return true;
        default: return c === m;
      }
    };
    const valid = (text, upTo) => {
      for (let i = 0; i < text.length; i++) {
        if (!fits(text[i], upTo[i])) {
          return false;
        }
      }
      return true;
    };
    let ok = v.length <= mask.length;
    if (ok && event.willCommit) {
      ok = v.length === mask.length && valid(v, mask);
    } else if (ok) {
      ok = valid(v, mask.slice(0, v.length));
    }
    if (!ok) {
      if (event.willCommit) {
        alert(`${INVALID} = "${mask}"`);
      }
      event.rc = false;
    }
  }

  function AFSimple_Calculate(kind, names) {
    const reduce = {
      SUM: (all) => all.reduce((a, b) => a + b, 0),
      PRD: (all) => all.reduce((a, b) => a * b, 1),
      AVG: (all) => all.reduce((a, b) => a + b, 0) / all.length,
      MIN: (all) => Math.min(...all),
      MAX: (all) => Math.max(...all),
    }[kind];
    if (!reduce) {
      throw new TypeError(`AFSimple_Calculate doesn't know ${kind}`);
    }
    const all = [];
    for (const name of AFMakeArrayFromList(names)) {
      const field = getField(name);
      if (!field) {
        continue;
      }
      for (const kid of field.getArray()) {
        all.push(AFMakeNumber(kid.value) ?? 0);
      }
    }
    const result = all.length ? reduce(all) : 0;
    G.event.value = Math.round(1e6 * result) / 1e6;
  }

  function AFRange_Validate(above, least, below, most) {
    const event = G.event;
    if (event.value === "" || event.value === null || event.value === undefined) {
      return;
    }
    const n = AFMakeNumber(event.value);
    if (n === null) {
      return;
    }
    let error = "";
    if (above && below) {
      if (n < least || n > most) {
        error = `Invalid value: must be greater than or equal to ${least} and less than or equal to ${most}.`;
      }
    } else if (above) {
      if (n < least) {
        error = `Invalid value: must be greater than or equal to ${least}.`;
      }
    } else if (below && n > most) {
      error = `Invalid value: must be less than or equal to ${most}.`;
    }
    if (error) {
      alert(error);
      event.rc = false;
    }
  }

  Object.assign(G, {
    AFMakeNumber,
    AFMergeChange,
    AFMakeArrayFromList,
    AFExtractNums(s) { return String(s ?? "").match(/\d+/g); },
    AFParseDateEx(text, order) { return parseDate(order, text); },
    AFNumber_Format,
    AFNumber_Keystroke,
    AFPercent_Format,
    AFPercent_Keystroke,
    AFDate_FormatEx,
    AFDate_KeystrokeEx,
    AFDate_Format(n) { AFDate_FormatEx(DATE_FORMATS[n] ?? DATE_FORMATS[0]); },
    AFDate_Keystroke(n) { AFDate_KeystrokeEx(DATE_FORMATS[n] ?? DATE_FORMATS[0]); },
    AFTime_Format(n) { AFDate_FormatEx(TIME_FORMATS[n] ?? TIME_FORMATS[0]); },
    AFTime_FormatEx: AFDate_FormatEx,
    AFTime_Keystroke(n) { AFDate_KeystrokeEx(TIME_FORMATS[n] ?? TIME_FORMATS[0]); },
    AFTime_KeystrokeEx: AFDate_KeystrokeEx,
    AFSpecial_Format,
    AFSpecial_Keystroke,
    AFSpecial_KeystrokeEx,
    AFSimple_Calculate,
    AFRange_Validate,
  });

  Object.defineProperty(G, "__handle", {
    value(text) {
      effects = [];
      try {
        const m = parse(text);
        switch (m.t) {
          case "init": init(m); break;
          case "opened": opened(); break;
          case "keystroke": keystroke(m); break;
          case "commit": commit(m); break;
          case "mouse": mouse(m); break;
          case "link":
            run(m.js, makeEvent("Mouse Up", "Link", G), "a link");
            break;
          case "page": page(m); break;
          case "sync": sync(m); break;
          case "timers": runTimers(); break;
        }
        finish();
      } catch (e) {
        log(`the viewer's scripts: ${e}${e && e.stack ? "\n" + e.stack : ""}`);
      }
      return stringify({ effects, timer: nextDelay() });
    },
  });
})();
