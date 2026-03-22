(function () {
  const hljsInstance = window.hljs;
  if (!hljsInstance) {
    return;
  }

  hljsInstance.registerLanguage("rhai", function (hljs) {
    return {
      name: "Rhai",
      aliases: ["rhai-script"],
      keywords: {
        keyword:
          "if else switch do while loop for in break continue return throw try catch fn private let const import export as and or not",
        literal: "true false null"
      },
      contains: [
        hljs.C_LINE_COMMENT_MODE,
        hljs.C_BLOCK_COMMENT_MODE,
        hljs.APOS_STRING_MODE,
        hljs.QUOTE_STRING_MODE,
        hljs.C_NUMBER_MODE,
        {
          className: "literal",
          begin: /#\{/,
          end: /\}/
        },
        {
          className: "function",
          beginKeywords: "fn",
          end: /[{;]/,
          excludeEnd: true,
          contains: [
            hljs.UNDERSCORE_TITLE_MODE,
            {
              className: "params",
              begin: /\(/,
              end: /\)/,
              contains: [
                hljs.C_LINE_COMMENT_MODE,
                hljs.C_BLOCK_COMMENT_MODE,
                hljs.APOS_STRING_MODE,
                hljs.QUOTE_STRING_MODE,
                hljs.C_NUMBER_MODE
              ]
            }
          ]
        }
      ]
    };
  });

  const highlightRhaiBlocks = function () {
    document
      .querySelectorAll("pre code.language-rhai, pre code.lang-rhai")
      .forEach(function (block) {
        if (typeof hljsInstance.highlightElement === "function") {
          block.removeAttribute("data-highlighted");
          hljsInstance.highlightElement(block);
          return;
        }

        if (typeof hljsInstance.highlightBlock === "function") {
          hljsInstance.highlightBlock(block);
        }
      });
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", highlightRhaiBlocks);
  } else {
    highlightRhaiBlocks();
  }
})();