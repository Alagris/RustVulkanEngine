
            struct VertexInput
            {
                float3 position : POSITION;
            };

            float4 VSMain(VertexInput input) : SV_Position
            {
                return float4(input.position, 1.0);
            }

            float4 PSMain() : SV_Target0
            {
                return float4(0.0, 0.0, 0.0, 1.0);
            }
        