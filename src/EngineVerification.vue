<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { shallowRef } from 'vue'

interface EngineReport {
  gitVersion: string
  binaryPath: string
  refFormat: string
  checks: { name: string; detail: string }[]
  objectBytesBefore: number
  objectBytesAfter: number
}

type Verification =
  | { status: 'idle' }
  | { status: 'running' }
  | { status: 'passed'; report: EngineReport }
  | { status: 'failed'; error: string }

// 此状态对应原生窗口中的一次引擎验收，结果只来自 Rust 命令。
const verification = shallowRef<Verification>({ status: 'idle' })

const formatBytes = new Intl.NumberFormat('zh-CN', {
  style: 'unit',
  unit: 'byte',
  unitDisplay: 'short',
})

async function runVerification() {
  verification.value = { status: 'running' }
  try {
    const report = await invoke<EngineReport>('verify_git_engine')
    verification.value = { status: 'passed', report }
  } catch (error) {
    verification.value = {
      status: 'failed',
      error: error instanceof Error ? error.message : String(error),
    }
  }
}
</script>

<template>
  <section class="diagnostics" aria-label="Git 引擎验证">
    <div class="diagnostic-intro">
      <p>验证随包 Git、引用事务、中断恢复和实际对象回收。</p>
      <button class="button" type="button" data-initial-focus :disabled="verification.status === 'running'" @click="runVerification">
        {{ verification.status === 'running' ? '正在验证…' : '运行验证' }}
      </button>
    </div>
    <template v-if="verification.status === 'passed'">
      <p role="status">{{ verification.report.gitVersion }} · {{ verification.report.refFormat }} · 已通过</p>
      <dl>
        <div v-for="check in verification.report.checks" :key="check.name">
          <dt>{{ check.name }}</dt><dd>{{ check.detail }}</dd>
        </div>
        <div>
          <dt>对象库缩减</dt><dd>{{ formatBytes.format(verification.report.objectBytesBefore - verification.report.objectBytesAfter) }}</dd>
        </div>
        <div><dt>随包程序</dt><dd><code>{{ verification.report.binaryPath }}</code></dd></div>
      </dl>
    </template>
    <p v-else-if="verification.status === 'failed'" class="inline-error" role="alert">{{ verification.error }}</p>
  </section>
</template>
