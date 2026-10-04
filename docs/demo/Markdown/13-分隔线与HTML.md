# 分隔线与 HTML

预期：三种分隔线都显示为横线；HTML 原样显示为源码文字（不在终端里渲染），编辑时不被改写。

## 分隔线

---

***

___

- - -

*  *  *

_____________________

## 容易混淆的情况

段落下一行的三条横线
---

上面是 Setext 二级标题，不是分隔线。

--

两条横线只是文字。

## HTML 块

<div align="center">
  <strong>居中的 HTML 块</strong>
</div>

<details>
<summary>点击展开</summary>

折叠内容里的 **Markdown**。

</details>

<table>
  <tr><th>HTML</th><th>表格</th></tr>
  <tr><td>a</td><td>b</td></tr>
</table>

<img src="assets/landscape.png" width="200" alt="HTML 图片标签不会预览">

## 注释

<!-- 这是一段 HTML 注释，GitHub 不显示 -->

<!--
多行注释
第二行
-->

注释之后的正文。

## 行内 HTML

换行标签<br>之后的文字，<b>加粗标签</b>，<a href="https://example.com">HTML 链接</a>。

## 不完整的 HTML

<div>
没有闭合的 div

<span>没有闭合的 span
