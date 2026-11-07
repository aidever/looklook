// 文件预览里的文字和代码（CodeMirror 6，只读）：按文件名语法高亮、行号、折叠、Ctrl/⌘+F 搜索、选中同词高亮、自动换行开关。
// 按需加载（FileBrowser 的 Preview 里 lazy），各语言的语法也是用到时才取（@codemirror/language-data）。
// `focus`（{ from, to }，从 1 开始）：那几行标出来，打开时滚到上面三分之一处（对话里“读的那段、改的地方”）。
import { Compartment, EditorState, RangeSetBuilder, Text } from '@codemirror/state';
import { Decoration, drawSelection, EditorView, highlightSpecialChars, keymap, lineNumbers } from '@codemirror/view';
import { defaultKeymap } from '@codemirror/commands';
import { bracketMatching, codeFolding, foldGutter, foldKeymap, HighlightStyle, LanguageDescription, syntaxHighlighting } from '@codemirror/language';
import { languages } from '@codemirror/language-data';
import { highlightSelectionMatches, search, searchKeymap } from '@codemirror/search';
import { tags as t } from '@lezer/highlight';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

// 颜色用 CSS 变量（styles.css 的 .cm-view），跟着系统的浅色 / 深色走
const v = (name) => `var(--cm-${name})`;
const style = HighlightStyle.define([
  { tag: [t.keyword, t.operatorKeyword, t.modifier, t.controlKeyword, t.definitionKeyword, t.moduleKeyword], color: v('kw') },
  { tag: [t.string, t.special(t.string), t.regexp, t.character], color: v('str') },
  { tag: [t.number, t.bool, t.null, t.atom, t.unit], color: v('num') },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: v('cmt'), fontStyle: 'italic' },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName], color: v('fn') },
  { tag: [t.typeName, t.className, t.namespace, t.standard(t.typeName)], color: v('type') },
  { tag: [t.propertyName, t.attributeName, t.labelName], color: v('prop') },
  { tag: [t.tagName, t.heading, t.strong], color: v('tag'), fontWeight: '600' },
  { tag: [t.meta, t.annotation, t.processingInstruction, t.escape], color: v('meta') },
  { tag: [t.link, t.url], color: v('fn'), textDecoration: 'underline' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: [t.inserted], color: v('ins') },
  { tag: [t.deleted, t.invalid], color: v('del') },
]);

const focusLine = Decoration.line({ class: 'cm-focus' });
function focusDeco(doc, from, to) {
  const b = new RangeSetBuilder();
  if (from) for (let n = from; n <= Math.min(to, doc.lines); n += 1) b.add(doc.line(n).from, doc.line(n).from, focusLine);
  return b.finish();
}

/** 这个文件名对应的语法（没有就纯文字） */
export const languageFor = (name) => LanguageDescription.matchFilename(languages, name);

export default function CodeView({ text, name, wrap, focus }) {
  const { t: tr } = useTranslation();
  const host = useRef(null);
  const view = useRef(null);
  const parts = useRef(null);
  const from = focus?.from ?? 0;
  const to = focus?.to ?? from;
  useEffect(() => {
    const wrapC = new Compartment();
    const langC = new Compartment();
    parts.current = { wrapC, langC };
    const doc = Text.of(text.split(/\r\n?|\n/));
    const state = EditorState.create({
      doc,
      extensions: [
        EditorState.readOnly.of(true),
        // 能放光标、能选、能 Ctrl+F，打不进字
        EditorView.contentAttributes.of({ 'aria-label': name, 'aria-readonly': 'true' }),
        lineNumbers(),
        foldGutter(),
        codeFolding(),
        highlightSpecialChars(),
        drawSelection(),
        bracketMatching(),
        highlightSelectionMatches(),
        search({ top: true }),
        syntaxHighlighting(style, { fallback: true }),
        keymap.of([...searchKeymap, ...foldKeymap, ...defaultKeymap]),
        EditorView.decorations.of(focusDeco(doc, from, to)),
        EditorState.phrases.of({
          Find: tr('cm.find'), Replace: tr('cm.replace'), next: tr('cm.next'), previous: tr('cm.previous'), all: tr('cm.all'),
          'match case': tr('cm.case'), regexp: tr('cm.regexp'), 'by word': tr('cm.word'), close: tr('cm.close'),
          'Fold line': tr('cm.fold'), 'Unfold line': tr('cm.unfold'), 'folded code': tr('cm.folded'), unfold: tr('cm.unfold'),
        }),
        wrapC.of(wrap ? EditorView.lineWrapping : []),
        langC.of([]),
      ],
    });
    const ed = new EditorView({ state, parent: host.current });
    view.current = ed;
    if (from) {
      const line = ed.state.doc.line(Math.min(from, ed.state.doc.lines));
      // 标出的几行放在上面三分之一，前面留几行看上下文
      requestAnimationFrame(() => ed.dispatch({ effects: EditorView.scrollIntoView(line.from, { y: 'start', yMargin: ed.scrollDOM.clientHeight / 4 }) }));
    }
    let alive = true;
    const lang = languageFor(name);
    // 太大的文件不上语法（解析慢、也没人从头读到尾）
    if (lang && text.length < 600_000) {
      lang.load().then((support) => alive && ed.dispatch({ effects: langC.reconfigure(support) })).catch(() => {});
    }
    return () => {
      alive = false;
      ed.destroy();
      view.current = null;
    };
  }, [text, name, from, to]);
  useEffect(() => {
    const ed = view.current;
    if (ed && parts.current) ed.dispatch({ effects: parts.current.wrapC.reconfigure(wrap ? EditorView.lineWrapping : []) });
  }, [wrap]);
  return <div ref={host} className="cm-view" />;
}
