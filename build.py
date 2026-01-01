# build.py
import os
import json
import asyncio
import aiosqlite
from openai import AsyncOpenAI
from dotenv import load_dotenv

# 加载环境变量
load_dotenv()

# 配置 DeepSeek
client = AsyncOpenAI(
    api_key=os.getenv("DEEPSEEK_API_KEY"), base_url="https://api.deepseek.com"
)
DB_PATH = "history-emotion.db"
OUTPUT_FILE = "static/data.json"


async def analyze_batch(records):
    """调用 DeepSeek 批量清洗数据"""
    if not records:
        return []

    # 构造 Prompt
    prompt_content = json.dumps(
        [{"id": r[0], "text": r[1]} for r in records], ensure_ascii=False
    )
    system_prompt = """
    你是一个数据清洗专家。请处理语音识别文本。
    任务：
    1. 滤除无效内容（如字幕残留、白噪音、乱码）。
    2. 提取1-3个情绪/内容关键词。
    3. 修正明显的同音错字。
    
    返回纯 JSON 列表（不要 Markdown），格式：
    [{"id": 1, "is_valid": true, "fixed_text": "...", "keywords": ["..."]}, ...]
    """

    print(f"正在通过 DeepSeek 分析 {len(records)} 条数据...")
    try:
        response = await client.chat.completions.create(
            model="deepseek-chat",
            messages=[
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": prompt_content},
            ],
            response_format={"type": "json_object"},
        )
        content = response.choices[0].message.content
        # 简单的清理逻辑，防止返回 Markdown 代码块
        if "```" in content:
            content = content.replace("```json", "").replace("```", "")

        data = json.loads(content)
        # 兼容返回格式可能是 {"results": [...]} 或直接是 [...]
        return data.get("results", data) if isinstance(data, dict) else data
    except Exception as e:
        print(f"AI 处理出错: {e}")
        return []


async def main():
    # 1. 读取数据库
    print(f"正在读取数据库: {DB_PATH} ...")
    async with aiosqlite.connect(DB_PATH) as db:
        # 这里你可以调整 limit，或者去掉 limit 获取全部
        cursor = await db.execute(
            "SELECT rowid, text, emotion, created_at FROM speech_results ORDER BY created_at DESC LIMIT 50"
        )
        rows = await cursor.fetchall()

    if not rows:
        print("数据库为空，未生成数据。")
        return

    # 2. AI 处理
    # 实际生产中如果数据量大，建议分批处理（chunking）
    ai_results = await analyze_batch(rows)

    # 建立 ID 映射表方便合并
    ai_map = {item["id"]: item for item in ai_results if "id" in item}

    # 3. 合并数据
    nodes = []
    for row in rows:
        row_id, raw_text, emotion, created_at = row
        processed = ai_map.get(row_id)

        # 只保留有效数据
        if processed and processed.get("is_valid", True):
            nodes.append(
                {
                    "id": row_id,
                    "text": processed.get("fixed_text", raw_text),
                    "original_text": raw_text,
                    "emotion": emotion,
                    "keywords": processed.get("keywords", []),
                    "created_at": created_at,
                }
            )

    # 4. 写入静态文件
    os.makedirs("static", exist_ok=True)
    with open(OUTPUT_FILE, "w", encoding="utf-8") as f:
        json.dump({"nodes": nodes}, f, ensure_ascii=False, indent=2)

    print(f"构建完成！数据已保存至 {OUTPUT_FILE}")
    print(f"共处理 {len(rows)} 条，生成有效节点 {len(nodes)} 个。")


if __name__ == "__main__":
    asyncio.run(main())
