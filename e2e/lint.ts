/**
 * Bans the ways an e2e test hides a race instead of fixing it, after alphaXiv's
 * `no-e2e-race-hiding`: retrying actions, sleeping, waiting on network quiet, forcing actions,
 * serial mode, and unconditional skips. `run.ts` checks every spec and fixture with it. @module
 */
import { readFileSync } from 'node:fs'
import ts from 'typescript'

const MESSAGES = {
  toPass: 'toPass() retries actions until one lands. Wait for the state that makes the action valid.',
  sleep: 'Do not sleep. Wait for the state that makes the next step valid: a locator, expect.poll, or an agent hold.',
  networkidle: 'networkidle is not readiness. Wait for what the page renders once ready.',
  force: 'force: true skips actionability checks. Wait for whatever covers or disables the target to go.',
  serial: 'Serial mode couples tests. Give each test its own setup.',
  skip: 'Do not skip unconditionally. Fix or delete the test, or skip on a condition with a reason.',
}

const FORCEABLE = new Set(['check', 'clear', 'click', 'dblclick', 'dragTo', 'fill', 'hover', 'selectOption', 'selectText', 'setChecked', 'tap', 'uncheck'])

const name = (node: ts.Node | undefined) =>
  node && (ts.isIdentifier(node) || ts.isStringLiteral(node) || ts.isPrivateIdentifier(node)) ? node.text : undefined

/** `test.describe.<member>` or `describe.<member>`. */
const onDescribe = (node: ts.PropertyAccessExpression) => {
  const object = node.expression
  return (ts.isPropertyAccessExpression(object) && object.name.text === 'describe') || (ts.isIdentifier(object) && object.text === 'describe')
}

export function lint(file: string, source = readFileSync(file, 'utf8')) {
  const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true)
  const problems: string[] = []
  const report = (node: ts.Node, message: keyof typeof MESSAGES) => {
    const { line, character } = tree.getLineAndCharacterOfPosition(node.getStart())
    problems.push(`${file}:${line + 1}:${character + 1} ${MESSAGES[message]}`)
  }

  const visit = (node: ts.Node) => {
    if (ts.isImportDeclaration(node) && /^(node:)?timers\/promises$/.test(name(node.moduleSpecifier) ?? '')) report(node, 'sleep')
    if (ts.isStringLiteral(node) && node.text === 'networkidle') report(node, 'networkidle')
    if (ts.isPropertyAssignment(node)) {
      const key = name(node.name)
      if (key === 'mode' && ts.isStringLiteral(node.initializer) && node.initializer.text === 'serial') report(node, 'serial')
      const call = node.parent.parent
      if (
        key === 'force' &&
        node.initializer.kind === ts.SyntaxKind.TrueKeyword &&
        ts.isCallExpression(call) &&
        ts.isPropertyAccessExpression(call.expression) &&
        FORCEABLE.has(call.expression.name.text)
      )
        report(node, 'force')
    }
    if (ts.isPropertyAccessExpression(node) && node.name.text === 'serial' && onDescribe(node)) report(node.name, 'serial')
    if (ts.isCallExpression(node)) {
      const callee = node.expression
      if (ts.isIdentifier(callee) && callee.text === 'setTimeout') {
        const executor = ts.findAncestor(node.parent, ts.isFunctionLike)
        if (executor && ts.isNewExpression(executor.parent) && name(executor.parent.expression) === 'Promise') report(callee, 'sleep')
      }
      if (ts.isPropertyAccessExpression(callee)) {
        const method = callee.name.text
        if (method === 'toPass') report(callee.name, 'toPass')
        if (method === 'waitForTimeout') report(callee.name, 'sleep')
        const [first] = node.arguments
        const unconditional = !first || ts.isStringLiteralLike(first) || ts.isTemplateExpression(first)
        if ((method === 'skip' || method === 'fixme') && (onDescribe(callee) || unconditional)) report(callee.name, 'skip')
      }
    }
    ts.forEachChild(node, visit)
  }
  visit(tree)
  return problems
}
