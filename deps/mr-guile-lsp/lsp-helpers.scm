;;; lsp-helpers.scm — thin adapter exposing Geiser analysis as `lsp-*` entry
;;; points for the Rust LSP server.
;;;
;;; Provenance & licensing:
;;;   Portions ported/adapted from rgherdt/scheme-lsp-server (MIT License,
;;;   Copyright (c) 2021 Ricardo G. Herdt), specifically its
;;;   `lsp-server/private/compat-guile-impl.scm` and `adapter-impl.scm`.
;;;   The JSON-RPC/LSP framing was removed (now handled in Rust via tower-lsp);
;;;   new code added (lsp-serve sentinel loop, lsp-check-syntax, lsp-load-file).
;;;   See deps/MODIFICATIONS.md entry #2 for the full adaptation record.
;;;
;;;   This file is licensed under the project's MIT license
;;;   (see LICENSE at the repo root). The MIT license of
;;;   the upstream source is retained by attribution.
;;;
;;; Output convention: every `lsp-*` query returns either #f (nothing found)
;;; or a simple, `write`-friendly S-expression the Rust side can parse:
;;;   - lsp-completions    -> list of string labels
;;;   - lsp-documentation  -> string docstring (or #f)
;;;   - lsp-signature      -> string signature (or #f)
;;;   - lsp-find-definition -> ((file . <str|#f>) (line . <int>)) (or #f)

(define-module (mr-guile-lsp lsp-helpers)
  #:export (lsp-completions
            lsp-documentation
            lsp-signature
            lsp-find-definition
            lsp-check-syntax
            lsp-load-file
            lsp-eval
            lsp-serve
            resolve-source-path)
  #:use-module (mr-guile-lsp geiser completion)
  #:use-module (mr-guile-lsp geiser doc)
  #:use-module (mr-guile-lsp geiser modules)
  #:use-module (mr-guile-lsp geiser xref)
  #:use-module (mr-guile-lsp geiser evaluation)
  #:use-module (ice-9 session)
  #:use-module (ice-9 rdelim)
  #:use-module (ice-9 regex)
  #:use-module (system base compile)
  #:use-module (srfi srfi-1))

;; --- module tracking ----------------------------------------------------
;; `lsp-load-file` records the modules a loaded buffer defines, so that
;; completion / documentation can reach bindings that live in user modules
;; (define-module forms) rather than only in the default (guile-user) module.
(define *loaded-modules* (make-fluid '()))

(define (remember-module! name)
  (let ((cur (fluid-ref *loaded-modules*)))
    (unless (member name cur)
      (fluid-set! *loaded-modules* (cons name cur)))))

(define (loaded-modules)
  (fluid-ref *loaded-modules*))

;; --- completions --------------------------------------------------------
;; Geiser `completions` uses apropos-internal over loaded bindings, returning
;; a sorted list of string labels matching the prefix. We also fold in the
;; exported bindings of user modules tracked via `lsp-load-file`, so that
;; symbols defined inside `define-module` forms are completable.
(define (module-exported-labels mod-name)
  ;; Return the string names of the exported bindings of MOD-NAME, or '().
  (let ((mod (and (module-name? mod-name) (resolve-module mod-name #f #:ensure #f))))
    (if (not mod)
        '()
        (let ((iface (module-public-interface mod)))
          (if (not iface)
              '()
              ;; module-map calls (proc name var) over every binding; keep names.
              (map symbol->string
                   (module-map (lambda (name _var) name) iface)))))))

(define (lsp-completions prefix)
  (let* ((base (completions prefix))
         (rx (string-append "^" (regexp-quote prefix)))
         (from-modules
          (append-map (lambda (m)
                        (filter (lambda (s) (string-match rx s))
                                (module-exported-labels m)))
                      (loaded-modules)))
         (all (delete-duplicates (append base from-modules))))
    (sort! all string<?)))

;; --- documentation ------------------------------------------------------
;; `symbol-documentation` returns a nested alist whose "docstring" entry holds
;; the doc text. Search the tree robustly (key may be string or symbol).
(define (find-docstring tree)
  (cond ((null? tree) #f)
        ((and (pair? tree) (or (equal? (car tree) "docstring")
                               (eq? (car tree) 'docstring)))
         (if (pair? (cdr tree)) (cadr tree) (cdr tree)))
        ((pair? tree)
         (or (find-docstring (car tree))
             (find-docstring (cdr tree))))
        (else #f)))

(define (lsp-documentation sym)
  ;; Look up the docstring in the default module first, then in each tracked
  ;; user module (define-module exports), so hover works on module-scoped
  ;; symbols too.
  (or (let ((doc (symbol-documentation sym)))
        (and doc (find-docstring doc)))
      (let loop ((mods (loaded-modules)))
        (if (null? mods)
            #f
            (let* ((mod-name (car mods))
                   (mod (and (module-name? mod-name)
                             (resolve-module mod-name #f #:ensure #f)))
                   (var (and mod (module-variable mod sym))))
              (if (and var (variable-bound? var))
                  (let ((val (variable-ref var)))
                    (or (and (procedure? val)
                             (procedure-documentation val))
                        (loop (cdr mods))))
                  (loop (cdr mods))))))))

;; --- signature ----------------------------------------------------------
;; `autodoc` returns ((name (args ((required ...) (optional ...) (key ...)))
;;                     (module ...))). Flatten the arg groups into a string.
(define (a-ref key alist)
  (let ((p (or (assq key alist)
               (assoc (symbol->string key) alist)
               (assoc key alist))))
    (and p (cdr p))))

(define (lsp-signature sym)
  (let ((result (autodoc (list sym))))
    (if (or (null? result) (null? (car result)))
        #f
        (let* ((entry (car result))
               (name (car entry))
               (props (cdr entry))
               (args (or (a-ref 'args props) '()))
               (req (or (a-ref 'required args) '()))
               (opt (or (a-ref 'optional args) '()))
               (keys (or (a-ref 'key args) '())))
          (format #f "(~a~a~a~a)"
                  name
                  (if (null? req) "" (string-append " " (join-syms req)))
                  (if (null? opt) "" (string-append " " (join-syms opt "()" )))
                  (if (null? keys) "" (string-append " " (join-syms keys))))))))

(define (join-syms lst . opt)
  (let ((wrap (if (pair? opt) (car opt) (lambda (s) s))))
    (string-join (map (lambda (x) (wrap (format #f "~a" x))) lst) " ")))

;; --- definition location ------------------------------------------------
;; `symbol-location` returns ((file . <str|sym|#f>) (line . <int>)) for source
;; bindings, or #f for C primitives / unresolvable symbols. The file may be a
;; relative path or a symbol; we resolve it to an absolute string so the Rust
;; side can build a file:// URL. Returns ((file . "<abs>") (line . N)) or #f.

(define (find-pair loc key)
  ;; Geiser's make-location uses STRING keys ("file"/"line"), but some callers
  ;; use symbol keys — try both forms.
  (or (and (symbol? key) (assq key loc))
      (and (symbol? key) (assoc (symbol->string key) loc))
      (assoc key loc)))

(define (alist-get-str loc key)
  ;; find `key` (symbol or string) and return its value as a string, or #f.
  (let ((p (find-pair loc key)))
    (and (pair? p)
         (let ((v (cdr p)))
           (cond ((string? v) v)
                 ((symbol? v) (symbol->string v))
                 (else #f))))))

(define (alist-get-num loc key)
  (let ((p (find-pair loc key)))
    (and (pair? p) (number? (cdr p)) (cdr p))))

(define (absolute-path? path)
  (and (string? path)
       (> (string-length path) 0)
       (char=? (string-ref path 0) #\/)))

(define (resolve-source-path path)
  ;; Resolve a possibly-relative Guile source path against %load-path.
  (if (absolute-path? path)
      (and (file-exists? path) path)
      (let loop ((dirs %load-path))
        (if (null? dirs)
            #f
            (let ((candidate (string-append (car dirs) "/" path)))
              (if (file-exists? candidate) candidate (loop (cdr dirs))))))))

(define (lsp-find-definition sym)
  (let* ((loc (symbol-location sym))
         (line (and loc (alist-get-num loc 'line))))
    (if (not line)
        #f
        (let* ((raw-file (and loc (alist-get-str loc 'file)))
               (abs (and raw-file
                         (> (string-length raw-file) 0)
                         (resolve-source-path raw-file))))
          ;; When the source file is unknown (e.g. user code loaded from a
          ;; string), return just the line; the Rust side falls back to the
          ;; current document's URI.
          (if abs
              `((file . ,abs) (line . ,line))
              `((line . ,line)))))))

;; --- diagnostics --------------------------------------------------------
;; Compile `file-path` capturing compiler warnings to a string. We use
;; `compile-file` (not `compile` on a string) because the unbound-variable
;; analysis only runs in the file-compile path (verified against Guile 3.0.11;
;; see project memory guile-compile-warnings). The Rust side writes the buffer
;; to a temp file, calls this, and parses the returned warning text.
;;
;; Returns an alist: ((warnings . "<raw text>") (error . "<msg>|#f")).
(define (lsp-check-syntax file-path)
  (define warn-port (open-output-string))
  (define error-msg #f)
  (parameterize ((current-warning-port warn-port))
    (catch #t
      (lambda () (compile-file file-path))
      (lambda (key . args)
        (set! error-msg
              (call-with-output-string
                (lambda (p) (print-exception p #f key args)))))))
  `((warnings . ,(get-output-string warn-port))
    (error . ,error-msg)))

;; --- load file into the REPL --------------------------------------------
;; Load `file-path` into the interaction environment so Geiser can introspect
;; the symbols it defines (for completion/hover/definition of user code). We
;; also scan for `define-module` forms and remember the modules they declare,
;; so completion/documentation can reach their exported bindings.
;; Errors are swallowed so a broken buffer never kills the REPL.

(define (file-module-names file-path)
  ;; Scan the top-level forms of FILE-PATH for (define-module (name ...) ...)
  ;; and return the list of module name lists. Reads the file as data; any read
  ;; error simply yields what was parsed so far.
  (define (form->module-name form)
    (and (pair? form)
         (or (eq? (car form) 'define-module)
             (equal? (car form) "define-module"))
         (pair? (cdr form))
         (let ((nm (cadr form)))
           (and (or (pair? nm) (null? nm))
                (every symbol? nm)
                nm))))
  (call-with-input-file file-path
    (lambda (port)
      (let loop ((acc '()))
        (catch #t
          (lambda ()
            (let ((form (read port)))
              (if (eof-object? form)
                  (reverse! acc)
                  (loop (let ((mn (form->module-name form)))
                          (if mn (cons mn acc) acc))))))
          (lambda args (reverse! acc)))))))

(define (lsp-load-file file-path)
  (catch #t
    (lambda ()
      (for-each remember-module! (file-module-names file-path))
      (ge:load-file file-path)
      #t)
    (lambda args #f)))

;; --- sentinel request/response loop -------------------------------------
;; The Rust parent writes one S-expr per line (the `lsp-*` call); we evaluate
;; it, `write` the result (single-line friendly), then emit a sentinel line so
;; the parent knows the response is complete. Any error becomes (_error msg).
(define *sentinel* "%%LSP-DONE%%")

(define (lsp-eval form)
  (if form
      (catch #t
        (lambda () (eval form (interaction-environment)))
        (lambda (key . args)
          (list '_error (format #f "~a: ~a" key args))))
      (list '_error "read-failed")))

(define (lsp-serve)
  (let loop ()
    (let ((line (read-line)))
      (unless (eof-object? line)
        (let* ((form (catch #t
                       (lambda () (read (open-input-string line)))
                       (lambda _ #f)))
               (result (lsp-eval form)))
          (write result)
          (newline)
          (display *sentinel*)
          (newline)
          (force-output))
        (loop)))))
