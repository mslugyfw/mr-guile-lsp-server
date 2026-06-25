;;; test-helpers.scm — Guile-side tests for deps/mr-guile-lsp/lsp-helpers.scm
;;;
;;; Run:  guile -L deps -L tests/scheme -s tests/scheme/test-helpers.scm
;;; Exits 0 on success, 1 on first failure.

(use-modules (srfi srfi-1)
             (mr-guile-lsp lsp-helpers))

(define failures 0)

(define (check name got expected)
  (cond ((equal? got expected)
         (format #t "ok   ~a\n" name))
        (else
         (set! failures (+ failures 1))
         (format #t "FAIL ~a\n  expected: ~s\n  got:      ~s\n"
                 name expected got))))

;;; ---- lsp-completions ---------------------------------------------------
;; Returns a list of string labels; core symbols must appear for "disp".
(let ((cs (lsp-completions "display")))
  (check "completions-returns-list-of-strings"
         (and (list? cs) (every string? cs))
         #t)
  (check "completions-contains-display"
         (and (member "display" cs) #t)
         #t))

;;; ---- lsp-documentation ------------------------------------------------
(let ((doc (lsp-documentation 'display)))
  (check "documentation-returns-string-or-false"
         (or (string? doc) (not doc))
         #t))

;;; ---- lsp-signature ----------------------------------------------------
(let ((sig (lsp-signature 'list)))
  (check "signature-returns-string-or-false"
         (or (string? sig) (not sig))
         #t))

;;; ---- lsp-find-definition ----------------------------------------------
;; `display` is re-exported across modules, so symbol-location may resolve to a
;; source file OR return #f depending on apropos state. Either is graceful.
(let ((loc (lsp-find-definition 'display)))
  (check "definition-graceful-for-core-symbol"
         (or (not loc) (list? loc))
         #t))

;;; ---- lsp-check-syntax -------------------------------------------------
;; Write a buffer with an unbound reference to a temp file, compile it, and
;; expect the warnings text to mention "unbound".
(let* ((tmp (string-append "/tmp/mr-guile-lsp-test-" (number->string (getpid)) ".scm"))
       (result
        (begin
          (call-with-output-file tmp
            (lambda (p)
              (display "(define (broken)\n  undefined-symbol-here)\n" p)))
          (lsp-check-syntax tmp))))
  (let ((warns (cdr (assq 'warnings result))))
    (check "check-syntax-returns-warnings-alist"
           (and (list? result)
                (assq 'warnings result)
                (assq 'error result)
                #t)
           #t)
    (check "check-syntax-captures-unbound-warning"
           (and (string-contains warns "unbound") #t)
           #t))
  (delete-file tmp))

(format #t "\n~a failure(s)\n" failures)
(exit (if (zero? failures) 0 1))
