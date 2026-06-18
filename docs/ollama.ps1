$body = @{
    model = "gemma4:12b-mlx"
    messages = @(
        @{role = "user"; content = "say this is a test"}
    ) 
} | ConvertTo-Json -Depth 5

$resp = Invoke-RestMethod `
    -Uri "http://192.168.0.240:11434/v1/chat/completions" `
    -Method Post `
    -ContentType "application/json" `
    -Body $body

$resp.choices[0].message.content
