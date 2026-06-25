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
  #:use-module (system base compile)
  #:use-module (srfi srfi-1))

;; --- completions --------------------------------------------------------
;; Geiser `completions` uses apropos-internal over loaded bindings, returning
;; a sorted list of string labels matching the prefix.
(define (lsp-completions prefix)
  (completions prefix))

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
  (let ((doc (symbol-documentation sym)))
    (and doc (find-docstring doc))))

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
;; the symbols it defines (for completion/hover/definition of user code).
;; Errors are swallowed so a broken buffer never kills the REPL.
(define (lsp-load-file file-path)
  (catch #t
    (lambda () (ge:load-file file-path) #t)
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
